//! Embedded CDN reverse proxy — ported from `motherbox-proxy` into the addon binary.
//!
//! Features:
//!   - CDN bypass via perfect browser header fingerprinting
//!   - Zero-copy async streaming (no buffering entire files)
//!   - Byte-range support for instant MP4 seeking
//!   - **Session system**: create sessions with all audio/quality/subtitle data,
//!     then switch between them via clean URLs — no restart needed
//!   - HLS playlist rewriting with proxy redirect
//!
//! API (called internally by `main.rs`, not as a standalone binary):
//!   - `ensure_proxy_running()` → starts once, returns bound port
//!   - `create_session(...)` → stores audio/quality/sub data, returns session ID

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use reqwest::Client;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{OnceCell, RwLock};

// ═══════════════════════════════════════════════════════════════════════════════
//  Session types
// ═══════════════════════════════════════════════════════════════════════════════

const MAX_SESSIONS: usize = 20;
const SESSION_TTL_SECS: u64 = 6 * 60 * 60; // 6 hours

#[derive(Clone, Debug)]
struct Session {
    title: String,
    created_at: Instant,
    /// Primary stream URL (highest quality original audio) — stored for session metadata
    #[allow(dead_code)]
    primary_url: String,
    headers: HashMap<String, String>,
    /// audio_name -> { quality_label -> url }
    audios: HashMap<String, HashMap<String, String>>,
    /// language -> subtitle_url
    subtitles: HashMap<String, String>,
}

type SessionStore = Arc<RwLock<HashMap<String, Session>>>;

// ═══════════════════════════════════════════════════════════════════════════════
//  Global proxy state — started once, lives forever
// ═══════════════════════════════════════════════════════════════════════════════

struct ProxyState {
    port: u16,
    sessions: SessionStore,
}

static PROXY: OnceCell<ProxyState> = OnceCell::const_new();

/// Start the embedded proxy server on a random port. Idempotent — only starts once.
/// Returns the port it's listening on.
pub async fn ensure_proxy_running() -> u16 {
    let state = PROXY
        .get_or_init(|| async {
            let sessions: SessionStore = Arc::new(RwLock::new(HashMap::new()));

            // Bind on 127.0.0.1:0 (random port)
            let addr = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = TcpListener::bind(addr)
                .await
                .expect("[proxy] Failed to bind proxy listener");
            let port = listener
                .local_addr()
                .expect("[proxy] Failed to get local addr")
                .port();

            let client = Arc::new(
                Client::builder()
                    .gzip(true)
                    .brotli(true)
                    .redirect(reqwest::redirect::Policy::limited(10))
                    .timeout(std::time::Duration::from_secs(120))
                    .pool_max_idle_per_host(8)
                    .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
                    .build()
                    .expect("[proxy] Failed to build reqwest client"),
            );

            // Background: accept connections
            let sessions_clone = sessions.clone();
            tokio::spawn(async move {
                eprintln!("[proxy] Embedded proxy listening on http://127.0.0.1:{}", port);
                loop {
                    match listener.accept().await {
                        Ok((stream, _peer)) => {
                            let io = TokioIo::new(stream);
                            let c = client.clone();
                            let s = sessions_clone.clone();
                            tokio::spawn(async move {
                                let service = service_fn(move |req| {
                                    let c = c.clone();
                                    let s = s.clone();
                                    async move { handle_request(req, c, s, port).await }
                                });
                                if let Err(err) = http1::Builder::new()
                                    .keep_alive(true)
                                    .serve_connection(io, service)
                                    .await
                                {
                                    if !err.to_string().contains("connection closed") {
                                        eprintln!("[proxy] Connection error: {}", err);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("[proxy] Accept error: {}", e);
                        }
                    }
                }
            });

            // Background: auto-cleanup expired sessions every 30 minutes
            let sessions_cleanup = sessions.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
                    let mut store = sessions_cleanup.write().await;
                    let before = store.len();
                    store.retain(|id, session| {
                        let age = session.created_at.elapsed().as_secs();
                        if age > SESSION_TTL_SECS {
                            eprintln!(
                                "[proxy] Removing expired session '{}' ('{}', age: {}h)",
                                id,
                                session.title,
                                age / 3600
                            );
                            false
                        } else {
                            true
                        }
                    });
                    let removed = before - store.len();
                    if removed > 0 {
                        eprintln!(
                            "[proxy] Removed {} expired sessions, {} remaining",
                            removed,
                            store.len()
                        );
                    }
                }
            });

            ProxyState { port, sessions }
        })
        .await;

    state.port
}

/// Create a session with all audio/quality/subtitle data.
/// Returns (session_id, proxy_port).
pub async fn create_session(
    audios: &HashMap<String, crate::models::AudioResult>,
    subtitles: &[crate::models::Subtitle],
    headers: &HashMap<String, String>,
    title: &str,
    primary_url: &str,
) -> (String, u16) {
    let state = PROXY.get().expect("[proxy] Proxy not initialized — call ensure_proxy_running first");
    let port = state.port;

    // Convert AudioResult -> HashMap<String, HashMap<String, String>>
    let mut audio_map: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (name, audio) in audios {
        audio_map.insert(name.clone(), audio.streams.clone());
    }

    // Convert subtitles to language -> url map
    let mut sub_map: HashMap<String, String> = HashMap::new();
    for sub in subtitles {
        sub_map.insert(sub.language.clone(), sub.url.clone());
    }

    // Generate session ID
    let id: String = {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("{:x}", t)
    };

    let session = Session {
        title: title.to_string(),
        created_at: Instant::now(),
        primary_url: primary_url.to_string(),
        headers: headers.clone(),
        audios: audio_map.clone(),
        subtitles: sub_map.clone(),
    };

    // Store + evict oldest if over limit
    {
        let mut store = state.sessions.write().await;
        store.insert(id.clone(), session);

        while store.len() > MAX_SESSIONS {
            let oldest_key = store
                .iter()
                .min_by_key(|(_, s)| s.created_at)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                eprintln!("[proxy] Evicting oldest session '{}' (limit: {})", key, MAX_SESSIONS);
                store.remove(&key);
            } else {
                break;
            }
        }
    }

    eprintln!(
        "[proxy] Session '{}' created: {} audio tracks, {} qualities total, {} subtitles",
        id,
        audio_map.len(),
        audio_map.values().map(|q| q.len()).sum::<usize>(),
        sub_map.len()
    );

    (id, port)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CDN bypass headers
// ═══════════════════════════════════════════════════════════════════════════════

const PLAYER_HOST: &str = "https://123movienow.cc";

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) \
    Chrome/124.0.0.0 Safari/537.36";

fn build_origin_headers(
    upstream_url: &str,
    session_headers: Option<&HashMap<String, String>>,
) -> Vec<(String, String)> {
    if let Some(sh) = session_headers {
        let mut headers = vec![
            ("User-Agent".into(), UA.to_string()),
            ("Accept".into(), "*/*".to_string()),
            ("Accept-Language".into(), "en-US,en;q=0.9".to_string()),
            ("DNT".into(), "1".to_string()),
            ("Connection".into(), "keep-alive".to_string()),
            ("Sec-Fetch-Dest".into(), "empty".to_string()),
            ("Sec-Fetch-Mode".into(), "cors".to_string()),
            ("Sec-Fetch-Site".into(), "cross-site".to_string()),
            (
                "Sec-CH-UA".into(),
                r#""Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99""#.to_string(),
            ),
            ("Sec-CH-UA-Mobile".into(), "?0".to_string()),
            ("Sec-CH-UA-Platform".into(), r#""Windows""#.to_string()),
        ];
        for (k, v) in sh {
            headers.push((k.clone(), v.clone()));
        }
        return headers;
    }

    let (referer, origin) = if upstream_url.contains("hakunaymatata.com")
        || upstream_url.contains("123movienow")
        || upstream_url.contains("moviebox")
    {
        (format!("{}/", PLAYER_HOST), PLAYER_HOST.to_string())
    } else if upstream_url.contains("cloudnestra.com")
        || upstream_url.contains("neonhorizon")
        || upstream_url.contains("orchidpixel")
        || upstream_url.contains("vidsrc")
    {
        (
            "https://cloudnestra.com/".into(),
            "https://cloudnestra.com".into(),
        )
    } else if let Ok(parsed) = url::Url::parse(upstream_url) {
        let o = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
        (format!("{}/", o), o)
    } else {
        (format!("{}/", PLAYER_HOST), PLAYER_HOST.to_string())
    };

    vec![
        ("User-Agent".into(), UA.to_string()),
        ("Accept".into(), "*/*".to_string()),
        ("Accept-Language".into(), "en-US,en;q=0.9".to_string()),
        ("Referer".into(), referer),
        ("Origin".into(), origin),
        ("DNT".into(), "1".to_string()),
        ("Connection".into(), "keep-alive".to_string()),
        ("Sec-Fetch-Dest".into(), "empty".to_string()),
        ("Sec-Fetch-Mode".into(), "cors".to_string()),
        ("Sec-Fetch-Site".into(), "cross-site".to_string()),
        (
            "Sec-CH-UA".into(),
            r#""Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99""#.to_string(),
        ),
        ("Sec-CH-UA-Mobile".into(), "?0".to_string()),
        ("Sec-CH-UA-Platform".into(), r#""Windows""#.to_string()),
    ]
}

// ═══════════════════════════════════════════════════════════════════════════════
//  URL encoding / decoding helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn encode_b64(url: &str) -> String {
    URL_SAFE_NO_PAD.encode(url.as_bytes())
}

fn decode_b64(token: &str) -> Option<String> {
    URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Content-type detection
// ═══════════════════════════════════════════════════════════════════════════════

fn content_type_for_url(url: &str) -> &'static str {
    let clean = url.split('?').next().unwrap_or(url).to_lowercase();
    if clean.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if clean.ends_with(".ts") {
        "video/mp2t"
    } else if clean.ends_with(".mp4") || clean.ends_with(".m4v") || clean.ends_with(".m4s") {
        "video/mp4"
    } else if clean.ends_with(".aac") || clean.ends_with(".m4a") {
        "audio/mp4"
    } else if clean.ends_with(".srt") {
        "text/plain; charset=utf-8"
    } else if clean.ends_with(".vtt") {
        "text/vtt; charset=utf-8"
    } else if clean.ends_with(".ass") || clean.ends_with(".ssa") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HLS playlist rewriting
// ═══════════════════════════════════════════════════════════════════════════════

fn rewrite_playlist(body: &str, base_url: &str, proxy_base: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);

    for line in body.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        // Rewrite URI="..." attributes (EXT-X-KEY, EXT-X-MAP, etc.)
        let mut rewritten = line.to_string();
        if rewritten.contains("URI=\"") {
            while let Some(start) = rewritten.find("URI=\"") {
                let attr_start = start + 5;
                if let Some(end) = rewritten[attr_start..].find('"') {
                    let uri = &rewritten[attr_start..attr_start + end].to_string();
                    let abs = resolve_url(base_url, uri);
                    let token = encode_b64(&abs);
                    let replacement = format!("URI=\"{}/b64/{}\"", proxy_base, token);
                    rewritten = format!(
                        "{}{}{}",
                        &rewritten[..start],
                        replacement,
                        &rewritten[attr_start + end + 1..]
                    );
                } else {
                    break;
                }
            }
        }

        if !trimmed.starts_with('#') {
            let abs = resolve_url(base_url, trimmed);
            let token = encode_b64(&abs);
            out.push_str(&format!("{}/b64/{}\n", proxy_base, token));
        } else {
            out.push_str(&rewritten);
            out.push('\n');
        }
    }

    out
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    if let Ok(base_parsed) = url::Url::parse(base) {
        if let Ok(resolved) = base_parsed.join(relative) {
            return resolved.to_string();
        }
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        relative.trim_start_matches('/')
    )
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HTTP response builders
// ═══════════════════════════════════════════════════════════════════════════════

fn empty_body() -> BoxBody<Bytes, Infallible> {
    Full::new(Bytes::new()).map_err(|e| match e {}).boxed()
}

fn full_body(data: Bytes) -> BoxBody<Bytes, Infallible> {
    Full::new(data).map_err(|e| match e {}).boxed()
}

fn text_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .body(full_body(Bytes::from(msg.to_string())))
        .unwrap()
}

fn json_response(status: StatusCode, json: &str) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "*")
        .body(full_body(Bytes::from(json.to_string())))
        .unwrap()
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Session route handlers
// ═══════════════════════════════════════════════════════════════════════════════

async fn handle_session_manifest(
    session_id: &str,
    sessions: &SessionStore,
    port: u16,
) -> Response<BoxBody<Bytes, Infallible>> {
    let store = sessions.read().await;
    let session = match store.get(session_id) {
        Some(s) => s,
        None => {
            return json_response(
                StatusCode::NOT_FOUND,
                r#"{"error":"Session not found"}"#,
            )
        }
    };

    let base = format!("http://127.0.0.1:{}", port);

    let mut audios = serde_json::Map::new();
    for (aname, qualities) in &session.audios {
        let mut q_map = serde_json::Map::new();
        for (quality, _url) in qualities {
            let proxy_url = format!(
                "{}/s/{}/{}/{}",
                base,
                session_id,
                urlencoding::encode(aname),
                urlencoding::encode(quality)
            );
            q_map.insert(quality.clone(), serde_json::Value::String(proxy_url));
        }
        audios.insert(aname.clone(), serde_json::Value::Object(q_map));
    }

    let mut subs = serde_json::Map::new();
    for (lang, _url) in &session.subtitles {
        let proxy_url = format!(
            "{}/s/{}/subs/{}",
            base,
            session_id,
            urlencoding::encode(lang)
        );
        subs.insert(lang.clone(), serde_json::Value::String(proxy_url));
    }

    let manifest = serde_json::json!({
        "sessionId": session_id,
        "audios": audios,
        "subtitles": subs,
    });

    json_response(StatusCode::OK, &manifest.to_string())
}

async fn resolve_session_stream(
    session_id: &str,
    audio_name: &str,
    quality: &str,
    sessions: &SessionStore,
) -> Result<(String, HashMap<String, String>), String> {
    let store = sessions.read().await;
    let session = match store.get(session_id) {
        Some(s) => s,
        None => return Err("Session not found".into()),
    };

    let qualities = match session.audios.get(audio_name) {
        Some(q) => q,
        None => {
            let available: Vec<&String> = session.audios.keys().collect();
            return Err(format!(
                "Audio '{}' not found. Available: {:?}",
                audio_name, available
            ));
        }
    };

    // Support "best" and "worst" shortcuts
    let url = if quality == "best" {
        let mut keys: Vec<(&String, i32)> = qualities
            .keys()
            .map(|k| {
                let v = k.trim_end_matches('p').parse::<i32>().unwrap_or(0);
                (k, v)
            })
            .collect();
        keys.sort_by(|a, b| b.1.cmp(&a.1));
        keys.first().and_then(|(k, _)| qualities.get(*k)).cloned()
    } else if quality == "worst" {
        let mut keys: Vec<(&String, i32)> = qualities
            .keys()
            .map(|k| {
                let v = k.trim_end_matches('p').parse::<i32>().unwrap_or(0);
                (k, v)
            })
            .collect();
        keys.sort_by(|a, b| a.1.cmp(&b.1));
        keys.first().and_then(|(k, _)| qualities.get(*k)).cloned()
    } else {
        qualities.get(quality).cloned()
    };

    match url {
        Some(u) => Ok((u, session.headers.clone())),
        None => {
            let available: Vec<&String> = qualities.keys().collect();
            Err(format!(
                "Quality '{}' not found for '{}'. Available: {:?}",
                quality, audio_name, available
            ))
        }
    }
}

async fn resolve_session_subtitle(
    session_id: &str,
    language: &str,
    sessions: &SessionStore,
) -> Result<(String, HashMap<String, String>), String> {
    let store = sessions.read().await;
    let session = match store.get(session_id) {
        Some(s) => s,
        None => return Err("Session not found".into()),
    };

    match session.subtitles.get(language) {
        Some(url) => Ok((url.clone(), session.headers.clone())),
        None => {
            let available: Vec<&String> = session.subtitles.keys().collect();
            Err(format!(
                "Subtitle '{}' not found. Available: {:?}",
                language, available
            ))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Core proxy — fetches upstream with CDN-bypass headers, streams back
// ═══════════════════════════════════════════════════════════════════════════════

async fn proxy_upstream(
    client: &Client,
    upstream_url: &str,
    incoming_req: &Request<hyper::body::Incoming>,
    proxy_base: &str,
    session_headers: Option<&HashMap<String, String>>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let is_playlist = upstream_url
        .split('?')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .ends_with(".m3u8");

    let mut req_builder = client.get(upstream_url);

    for (k, v) in build_origin_headers(upstream_url, session_headers) {
        req_builder = req_builder.header(k, v);
    }

    // Forward Range header (critical for MP4 seeking)
    if let Some(range) = incoming_req.headers().get("range") {
        if let Ok(range_str) = range.to_str() {
            req_builder = req_builder.header("Range", range_str);
        }
    }

    let upstream_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[proxy] UPSTREAM ERROR: {} → {}",
                &upstream_url[..upstream_url.len().min(80)],
                e
            );
            return text_response(StatusCode::BAD_GATEWAY, &format!("Upstream error: {}", e));
        }
    };

    let status = upstream_resp.status();
    let upstream_status = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        StatusCode::PARTIAL_CONTENT
    } else if status.is_success() {
        StatusCode::OK
    } else {
        eprintln!(
            "[proxy] UPSTREAM {} → {}",
            status.as_u16(),
            &upstream_url[..upstream_url.len().min(80)]
        );
        return text_response(
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &format!("Upstream returned {}", status),
        );
    };

    let content_length = upstream_resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let content_range = upstream_resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let accept_ranges = upstream_resp
        .headers()
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ct = content_type_for_url(upstream_url);

    // HLS: buffer + rewrite
    if is_playlist {
        let body_bytes = match upstream_resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return text_response(StatusCode::BAD_GATEWAY, &format!("Read error: {}", e));
            }
        };

        let sniff = &body_bytes[..body_bytes.len().min(512)];
        if sniff.starts_with(b"<!") || sniff.starts_with(b"<html") || sniff.starts_with(b"<HTML")
        {
            eprintln!("[proxy] CDN returned HTML instead of HLS — blocked!");
            return text_response(StatusCode::BAD_GATEWAY, "CDN blocked: returned HTML page");
        }

        let body_str = String::from_utf8_lossy(&body_bytes);
        let rewritten = rewrite_playlist(&body_str, upstream_url, proxy_base);

        return Response::builder()
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Content-Length", rewritten.len().to_string())
            .header("Access-Control-Allow-Origin", "*")
            .header(
                "Access-Control-Expose-Headers",
                "Content-Length, Content-Range",
            )
            .header("Cache-Control", "no-cache")
            .body(full_body(Bytes::from(rewritten)))
            .unwrap();
    }

    // MP4 / TS / subtitle: zero-copy streaming
    let mut resp_builder = Response::builder()
        .status(upstream_status)
        .header("Content-Type", ct)
        .header("Access-Control-Allow-Origin", "*")
        .header(
            "Access-Control-Expose-Headers",
            "Content-Length, Content-Range, Accept-Ranges",
        )
        .header("Cache-Control", "public, max-age=3600");

    if let Some(cl) = content_length {
        resp_builder = resp_builder.header("Content-Length", cl);
    }
    if let Some(cr) = content_range {
        resp_builder = resp_builder.header("Content-Range", cr);
    }
    resp_builder = resp_builder.header(
        "Accept-Ranges",
        accept_ranges.as_deref().unwrap_or("bytes"),
    );

    let stream = upstream_resp
        .bytes_stream()
        .map(|result| -> Result<Frame<Bytes>, Infallible> {
            match result {
                Ok(chunk) => Ok(Frame::data(chunk)),
                Err(e) => {
                    eprintln!("[proxy] Stream chunk error: {}", e);
                    Ok(Frame::data(Bytes::new()))
                }
            }
        });

    let body = StreamBody::new(stream);
    let boxed: BoxBody<Bytes, Infallible> = BodyExt::boxed(body);

    resp_builder.body(boxed).unwrap()
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Main request router
// ═══════════════════════════════════════════════════════════════════════════════

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    client: Arc<Client>,
    sessions: SessionStore,
    port: u16,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let method = req.method().clone();

    // CORS preflight
    if method == Method::OPTIONS {
        return Ok(Response::builder()
            .status(200)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
            .header("Access-Control-Allow-Headers", "Range, Content-Type")
            .header("Access-Control-Max-Age", "86400")
            .body(empty_body())
            .unwrap());
    }

    let proxy_base = format!("http://127.0.0.1:{}", port);

    // ════════════════════════════════════════════════════════════════════════
    //  SESSION ROUTES
    // ════════════════════════════════════════════════════════════════════════

    // /s/{id}/... — session routes
    if path.starts_with("/s/") {
        let rest = &path[3..]; // after "/s/"
        let parts: Vec<&str> = rest.splitn(4, '/').collect();

        if parts.is_empty() {
            return Ok(text_response(StatusCode::BAD_REQUEST, "Missing session ID"));
        }

        let session_id = parts[0];

        // GET /s/{id}/manifest
        if parts.len() >= 2 && parts[1] == "manifest" {
            return Ok(handle_session_manifest(session_id, &sessions, port).await);
        }

        // GET /s/{id}/subs/{language}
        if parts.len() >= 3 && parts[1] == "subs" {
            let language = urlencoding::decode(parts[2])
                .unwrap_or_default()
                .into_owned();
            match resolve_session_subtitle(session_id, &language, &sessions).await {
                Ok((url, hdrs)) => {
                    return Ok(
                        proxy_upstream(&client, &url, &req, &proxy_base, Some(&hdrs)).await,
                    );
                }
                Err(e) => {
                    return Ok(json_response(
                        StatusCode::NOT_FOUND,
                        &format!(r#"{{"error":"{}"}}"#, e),
                    ));
                }
            }
        }

        // GET /s/{id}/{audio}/{quality}
        if parts.len() >= 3 {
            let audio_name = urlencoding::decode(parts[1])
                .unwrap_or_default()
                .into_owned();
            let quality = urlencoding::decode(parts[2])
                .unwrap_or_default()
                .into_owned();
            match resolve_session_stream(session_id, &audio_name, &quality, &sessions).await {
                Ok((url, hdrs)) => {
                    eprintln!(
                        "[proxy] {} → {} / {} → {}",
                        session_id,
                        audio_name,
                        quality,
                        &url[..url.len().min(60)]
                    );
                    return Ok(
                        proxy_upstream(&client, &url, &req, &proxy_base, Some(&hdrs)).await,
                    );
                }
                Err(e) => {
                    return Ok(json_response(
                        StatusCode::NOT_FOUND,
                        &format!(r#"{{"error":"{}"}}"#, e),
                    ));
                }
            }
        }

        return Ok(text_response(
            StatusCode::BAD_REQUEST,
            "Invalid session path. Use: /s/{id}/{audio}/{quality} or /s/{id}/subs/{lang}",
        ));
    }

    // ════════════════════════════════════════════════════════════════════════
    //  DIRECT PROXY ROUTES (stateless, no session needed)
    // ════════════════════════════════════════════════════════════════════════

    // GET /stream?url=<url-encoded>
    if path == "/stream" {
        let upstream_url = extract_query_param(&query, "url");
        if upstream_url.is_empty() {
            return Ok(text_response(
                StatusCode::BAD_REQUEST,
                "Missing ?url= parameter",
            ));
        }
        return Ok(proxy_upstream(&client, &upstream_url, &req, &proxy_base, None).await);
    }

    // GET /b64/<base64url-encoded-url>
    if path.starts_with("/b64/") {
        let token = &path[5..];
        match decode_b64(token) {
            Some(upstream_url) => {
                return Ok(
                    proxy_upstream(&client, &upstream_url, &req, &proxy_base, None).await,
                );
            }
            None => {
                return Ok(text_response(StatusCode::BAD_REQUEST, "Invalid base64 token"));
            }
        }
    }

    // GET /sub?url=<url-encoded>
    if path == "/sub" {
        let upstream_url = extract_query_param(&query, "url");
        if upstream_url.is_empty() {
            return Ok(text_response(
                StatusCode::BAD_REQUEST,
                "Missing ?url= parameter",
            ));
        }
        return Ok(proxy_upstream(&client, &upstream_url, &req, &proxy_base, None).await);
    }

    // GET / — health
    if path == "/" || path.is_empty() {
        return Ok(json_response(
            StatusCode::OK,
            r#"{"status":"ok","service":"motherbox-proxy-embedded"}"#,
        ));
    }

    Ok(text_response(StatusCode::NOT_FOUND, "Not found"))
}

fn extract_query_param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return urlencoding::decode(v).unwrap_or_default().into_owned();
            }
        }
    }
    String::new()
}
