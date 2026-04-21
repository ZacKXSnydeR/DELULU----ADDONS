//! MotherBox — self-contained MovieBox stream resolver.
//! JSON-RPC 2.0 over STDIN/STDOUT, per the Delulu binary add-on protocol.
//! All diagnostic logging goes to STDERR (safe for RPC).

mod auth;
mod http;
mod models;
mod moviebox;
mod proxy;
mod state;
mod tmdb;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use models::{AudioResult, JsonRpcRequest, JsonRpcResponse, ResolveStreamParams, StreamResult};
use serde_json::{json, Value};

use auth::PLAYER_HOST;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Force UTF-8 output on Windows — critical for non-ASCII subtitle languages
    #[cfg(target_os = "windows")]
    {
        unsafe {
            // SetConsoleOutputCP(65001) — CP_UTF8
            extern "system" {
                fn SetConsoleOutputCP(codepage: u32) -> i32;
            }
            SetConsoleOutputCP(65001);
        }
    }

    eprintln!("[motherbox] v{} starting — MovieBox resolver", env!("CARGO_PKG_VERSION"));

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut handle = stdin.lock();
    let mut out = stdout.lock();
    let mut line = String::new();

    while handle.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
            match req.method.as_str() {
                "resolveStream" => {
                    if let Ok(params) = serde_json::from_value::<ResolveStreamParams>(req.params) {
                        let result = resolve_stream(params).await;
                        send_response(&mut out, req.id, result);
                    } else {
                        send_error(&mut out, req.id, -32602, "Invalid params for resolveStream");
                    }
                }
                "healthCheck" => {
                    let result = json!({
                        "ok": true,
                        "version": env!("CARGO_PKG_VERSION"),
                        "provider": "moviebox",
                        "latencyMs": 0
                    });
                    send_response(&mut out, req.id, result);
                }
                _ => {
                    send_error(&mut out, req.id, -32601, &format!("Unknown method: {}", req.method));
                }
            }
        }
        line.clear();
    }

    Ok(())
}

async fn resolve_stream(params: ResolveStreamParams) -> StreamResult {
    let started = std::time::Instant::now();
    eprintln!(
        "[resolve] {} tmdb={} season={:?} episode={:?}",
        params.media_type, params.tmdb_id, params.season, params.episode
    );

    let client = http::build_client();

    // 0. Check daily limit
    let st = state::load_state_with_env_fallback();
    if st.is_daily_limit_hit() {
        eprintln!("[resolve] Daily limit hit today — results may be restricted");
    }

    // 1. TMDB API key
    let api_key = if !st.tmdb_api_key.is_empty() {
        st.tmdb_api_key.clone()
    } else {
        std::env::var("TMDB_API_KEY").unwrap_or_default()
    };

    if api_key.is_empty() {
        return StreamResult::error("CONFIG_ERROR", "No TMDB API key configured");
    }

    // 2. Fetch TMDB metadata (with offset calculation for Anime)
    let tmdb = match tmdb::fetch_tmdb_info(
        &client, 
        params.tmdb_id, 
        &params.media_type, 
        params.season, 
        params.episode, 
        &api_key
    ).await {
        Ok(t) => t,
        Err(e) => return StreamResult::error("TMDB_ERROR", &e.to_string()),
    };

    // 3. Authenticate
    let auth = match auth::ensure_auth(&client).await {
        Some(a) => a,
        None => return StreamResult::error("AUTH_FAILED", "Could not obtain MovieBox credentials"),
    };

    // 4. Search MovieBox (using Jaro-Winkler scoring)
    let path = match moviebox::search_detail_path(&client, &tmdb).await {
        Ok(p) => p,
        Err(e) => return StreamResult::error("NOT_FOUND", &e.to_string()),
    };

    // 5. Get detail
    let detail = match moviebox::get_detail(&client, &path).await {
        Ok(d) => d,
        Err(e) => return StreamResult::error("DETAIL_ERROR", &e.to_string()),
    };

    // 6. Map episode indices (with support for absolute numbering)
    let (se, ep) = if tmdb.media_type == "tv" {
        moviebox::map_episode_indices(&tmdb, &detail.seasons)
    } else {
        (0, 0)
    };

    // 7. Build version list (original + dubs)
    let mut versions: Vec<(String, String, String)> = vec![(
        "Original Audio".to_string(),
        detail.subject_id.clone(),
        detail.detail_path.clone(),
    )];
    for d in &detail.dubs {
        if let (Some(name), Some(sid), Some(dp)) = (
            d["lanName"].as_str(),
            d["subjectId"].as_str(),
            d["detailPath"].as_str(),
        ) {
            versions.push((name.to_string(), sid.to_string(), dp.to_string()));
        }
    }

    eprintln!("[resolve] Extracting {} audio version(s)...", versions.len());

    // 8. Concurrent extraction
    let mut handles = Vec::new();
    for (v_name, sid, dp) in versions {
        let client_cl = client.clone();
        let auth_cl = auth.clone();
        handles.push(tokio::spawn(async move {
            let res = moviebox::get_play_links_with_retry(&client_cl, &sid, &dp, se, ep, &auth_cl).await;
            (v_name, res)
        }));
    }

    let mut audio_results: HashMap<String, AudioResult> = HashMap::new();
    let mut best_media_id = None;
    let mut primary_stream_url = None;
    let mut extraction_errors: Vec<String> = Vec::new();

    for handle in handles {
        if let Ok((v_name, res)) = handle.await {
            let mut streams_map = HashMap::new();
            if let Ok((links, _fresh_auth)) = res {
                for s in &links {
                    let res_label = format!("{}p", s["resolutions"].as_str().unwrap_or("unknown"));
                    if let Some(url) = s["url"].as_str() {
                        streams_map.insert(res_label, url.to_string());
                        if best_media_id.is_none() {
                            best_media_id = s["id"].as_str().map(|s| s.to_string());
                        }
                    }
                }
            } else if let Err(err) = res {
                extraction_errors.push(format!("{}: {}", v_name, err));
            }

            if v_name == "Original Audio" && primary_stream_url.is_none() {
                let mut keys: Vec<_> = streams_map.keys().cloned().collect();
                keys.sort_by(|a, b| {
                    let a_val = a.trim_end_matches('p').parse::<i32>().unwrap_or(0);
                    let b_val = b.trim_end_matches('p').parse::<i32>().unwrap_or(0);
                    b_val.cmp(&a_val)
                });
                if let Some(best_res) = keys.first() {
                    primary_stream_url = streams_map.get(best_res).cloned();
                }
            }

            if !streams_map.is_empty() {
                audio_results.insert(v_name, AudioResult { streams: streams_map });
            }
        }
    }

    // 9. Subtitles
    let subtitles = if let Some(ref mid) = best_media_id {
        moviebox::get_captions(&client, &detail.subject_id, &detail.detail_path, Some(mid))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let elapsed = started.elapsed().as_millis();
    let success = !audio_results.is_empty() && primary_stream_url.is_some();
    if !success {
        let hint = if extraction_errors.is_empty() {
            "No playable links found".to_string()
        } else {
            extraction_errors.join(" | ")
        };
        return StreamResult::error("NO_PLAYABLE_STREAMS", &hint);
    }

    // 10. Start proxy
    let proxy_port = proxy::ensure_proxy_running().await;
    let mut cdn_headers = HashMap::new();
    cdn_headers.insert("Referer".into(), format!("{}/", PLAYER_HOST));
    cdn_headers.insert("Origin".into(), PLAYER_HOST.into());

    // 11. Create session
    let raw_primary = primary_stream_url.as_deref().unwrap_or("");
    let (session_id, _) = proxy::create_session(
        &audio_results,
        &subtitles,
        &cdn_headers,
        &tmdb.title,
        raw_primary,
    ).await;

    // 12. Rewrite URLs
    let base = format!("http://127.0.0.1:{}", proxy_port);
    let proxied_primary = format!("{}/s/{}/Original%20Audio/best", base, session_id);

    let mut proxied_audios = HashMap::new();
    for (name, audio) in &audio_results {
        let mut proxied_streams = HashMap::new();
        for (quality, _) in &audio.streams {
            proxied_streams.insert(
                quality.clone(),
                format!(
                    "{}/s/{}/{}/{}",
                    base,
                    session_id,
                    urlencoding::encode(name),
                    urlencoding::encode(quality)
                ),
            );
        }
        proxied_audios.insert(name.clone(), AudioResult { streams: proxied_streams });
    }

    let proxied_subs: Vec<models::Subtitle> = subtitles
        .iter()
        .map(|s| models::Subtitle {
            url: format!("{}/s/{}/subs/{}", base, session_id, urlencoding::encode(&s.language)),
            language: s.language.clone(),
        })
        .collect();

    eprintln!("[resolve] Done in {}ms — success={} proxy={}", elapsed, success, proxy_port);

    StreamResult {
        success,
        stream_url: Some(proxied_primary),
        headers: Some(HashMap::new()),
        subtitles: if proxied_subs.is_empty() { None } else { Some(proxied_subs) },
        audios: if proxied_audios.is_empty() { None } else { Some(proxied_audios) },
        proxy_port: Some(proxy_port),
        session_id: Some(session_id),
        self_proxy: Some(true),
        ..Default::default()
    }
}

fn send_response<T: serde::Serialize>(out: &mut io::StdoutLock, id: Option<Value>, result: T) {
    let resp = JsonRpcResponse {
        id,
        jsonrpc: "2.0".to_string(),
        result: Some(result),
        error: None,
    };
    if let Ok(json) = serde_json::to_string(&resp) {
        let _ = writeln!(out, "{}", json);
        let _ = out.flush();
    }
}

fn send_error(out: &mut io::StdoutLock, id: Option<Value>, code: i32, message: &str) {
    let resp = JsonRpcResponse::<Value> {
        id,
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(models::JsonRpcError {
            code,
            message: message.to_string(),
        }),
    };
    if let Ok(json) = serde_json::to_string(&resp) {
        let _ = writeln!(out, "{}", json);
        let _ = out.flush();
    }
}
