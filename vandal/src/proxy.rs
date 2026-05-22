use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use reqwest::Client;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{OnceCell, RwLock};

const MAX_SESSIONS: usize = 50;

#[derive(Clone, Debug)]
struct Session {
    audios: HashMap<String, HashMap<String, String>>,
    headers: HashMap<String, String>,
    created_at: Instant,
}

struct ProxyState {
    port: u16,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

static PROXY: OnceCell<ProxyState> = OnceCell::const_new();

pub async fn ensure_proxy_running() -> u16 {
    let state = PROXY.get_or_init(|| async {
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = TcpListener::bind(addr).await.expect("Failed to bind proxy listener");
        let port = listener.local_addr().unwrap().port();
        
        let client = Arc::new(
            Client::builder()
                .danger_accept_invalid_certs(true)
                .pool_max_idle_per_host(10)
                .build()
                .unwrap(),
        );

        let sessions_clone = sessions.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let io = TokioIo::new(stream);
                    let c = client.clone();
                    let s = sessions_clone.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req| {
                            let c = c.clone();
                            let s = s.clone();
                            async move { handle_request(req, c, s, port).await }
                        });
                        let _ = http1::Builder::new()
                            .keep_alive(true)
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        });

        ProxyState { port, sessions }
    }).await;
    state.port
}

pub async fn create_session(
    audios: &HashMap<String, HashMap<String, String>>,
    headers: &HashMap<String, String>,
) -> String {
    let state = PROXY.get().expect("Proxy not initialized");
    let id = format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis());
    let session = Session {
        audios: audios.clone(),
        headers: headers.clone(),
        created_at: Instant::now(),
    };
    
    let mut store = state.sessions.write().await;
    store.insert(id.clone(), session);
    
    while store.len() > MAX_SESSIONS {
        if let Some(key) = store.iter().min_by_key(|(_, s)| s.created_at).map(|(k, _)| k.clone()) {
            store.remove(&key);
        }
    }
    
    id
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    client: Arc<Client>,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    port: u16,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    
    if method == Method::OPTIONS {
        return Ok(Response::builder()
            .status(200)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS")
            .header("Access-Control-Allow-Headers", "Range, Content-Type")
            .header("Access-Control-Max-Age", "86400")
            .body(Full::new(Bytes::new()).map_err(|e| match e {}).boxed())
            .unwrap());
    }

    let proxy_base = format!("http://127.0.0.1:{}", port);

    if path.starts_with("/s/") {
        let parts: Vec<&str> = path[3..].split('/').collect();
        if parts.is_empty() { return Ok(text_response(400, "Missing session ID")); }
        let session_id = parts[0];

        let store = sessions.read().await;
        let session = match store.get(session_id) {
            Some(s) => s.clone(),
            None => return Ok(text_response(404, "Session not found")),
        };
        drop(store);

        if parts.len() >= 3 {
            let audio_name = urlencoding::decode(parts[1]).unwrap_or_default().into_owned();
            let quality = urlencoding::decode(parts[2]).unwrap_or_default().into_owned();
            
            if let Some(qualities) = session.audios.get(&audio_name) {
                if let Some(target_url) = qualities.get(&quality) {
                    return Ok(proxy_upstream(&client, target_url, &req, &proxy_base, &session.headers).await);
                }
            }
            return Ok(text_response(404, "Audio or Quality not found"));
        }
    } else if path.starts_with("/b64/") {
        let token = &path[5..];
        if let Some(url) = decode_b64(token) {
            return Ok(proxy_upstream(&client, &url, &req, &proxy_base, &HashMap::new()).await);
        }
    }

    Ok(text_response(404, "Not Found"))
}

async fn proxy_upstream(
    client: &Client,
    upstream_url: &str,
    incoming_req: &Request<hyper::body::Incoming>,
    proxy_base: &str,
    session_headers: &HashMap<String, String>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let is_playlist = upstream_url.split('?').next().unwrap_or("").to_lowercase().ends_with(".m3u8");

    let mut req_builder = client.get(upstream_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .header("Accept", "*/*")
        .header("Connection", "keep-alive");

    for (k, v) in session_headers {
        req_builder = req_builder.header(k, v);
    }
    
    if !session_headers.contains_key("Referer") {
        req_builder = req_builder.header("Referer", "https://player.videasy.net/");
        req_builder = req_builder.header("Origin", "https://player.videasy.net");
    }

    if let Some(range) = incoming_req.headers().get("range") {
        req_builder = req_builder.header("Range", range.to_str().unwrap());
    }

    let upstream_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => return text_response(502, &format!("Upstream error: {}", e)),
    };

    let status = upstream_resp.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return text_response(status.as_u16(), &format!("Upstream returned {}", status));
    }

    let ct = content_type_for_url(upstream_url);

    if is_playlist {
        let body_bytes = upstream_resp.bytes().await.unwrap_or_default();
        let body_str = String::from_utf8_lossy(&body_bytes);
        let rewritten = rewrite_playlist(&body_str, upstream_url, proxy_base);
        
        return Response::builder()
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "no-cache")
            .body(Full::new(Bytes::from(rewritten)).map_err(|e| match e {}).boxed())
            .unwrap();
    }

    let mut builder = Response::builder()
        .status(if status == reqwest::StatusCode::PARTIAL_CONTENT { 206 } else { 200 })
        .header("Content-Type", ct)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "Content-Length, Content-Range, Accept-Ranges");

    if let Some(cl) = upstream_resp.headers().get("content-length") {
        builder = builder.header("Content-Length", cl.to_str().unwrap());
    }
    if let Some(cr) = upstream_resp.headers().get("content-range") {
        builder = builder.header("Content-Range", cr.to_str().unwrap());
    }
    builder = builder.header("Accept-Ranges", "bytes");

    let stream = upstream_resp.bytes_stream().map(|res| {
        match res {
            Ok(b) => Ok(Frame::data(b)),
            Err(_) => Ok(Frame::data(Bytes::new())),
        }
    });

    builder.body(BodyExt::boxed(StreamBody::new(stream))).unwrap()
}

fn rewrite_playlist(body: &str, base_url: &str, proxy_base: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { out.push('\n'); continue; }

        let mut rewritten = line.to_string();

        // Hack to bypass strict codec checking in hls.js for HEVC/4K streams
        if rewritten.starts_with("#EXT-X-STREAM-INF:") || rewritten.starts_with("#EXT-X-I-FRAME-STREAM-INF:") || rewritten.starts_with("#EXT-X-MEDIA:") {
            if let Some(codec_idx) = rewritten.find("CODECS=\"") {
                if let Some(end_idx) = rewritten[codec_idx + 8..].find('"') {
                    let full_attr = &rewritten[codec_idx..codec_idx + 8 + end_idx + 1];
                    rewritten = rewritten.replace(full_attr, "");
                    rewritten = rewritten.replace(",,", ",");
                    rewritten = rewritten.replace(":,", ":");
                    if rewritten.ends_with(',') {
                        rewritten.pop();
                    }
                }
            }
        }

        if rewritten.contains("URI=\"") {
            let mut search_start = 0;
            while let Some(start) = rewritten[search_start..].find("URI=\"") {
                let actual_start = search_start + start;
                let attr_start = actual_start + 5;
                if let Some(end) = rewritten[attr_start..].find('"') {
                    let uri = &rewritten[attr_start..attr_start + end].to_string();
                    let abs = resolve_url(base_url, uri);
                    let token = encode_b64(&abs);
                    let replacement = format!("{}/b64/{}", proxy_base, token);
                    rewritten = format!("{}{}{}", &rewritten[..attr_start], replacement, &rewritten[attr_start+end..]);
                    search_start = attr_start + replacement.len() + 1;
                } else { break; }
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
    if relative.starts_with("http://") || relative.starts_with("https://") { return relative.to_string(); }
    if let Ok(base_parsed) = url::Url::parse(base) {
        if let Ok(resolved) = base_parsed.join(relative) { return resolved.to_string(); }
    }
    format!("{}/{}", base.trim_end_matches('/'), relative.trim_start_matches('/'))
}

fn content_type_for_url(url: &str) -> &'static str {
    let clean = url.split('?').next().unwrap_or(url).to_lowercase();
    if clean.ends_with(".m3u8") { "application/vnd.apple.mpegurl" }
    else if clean.ends_with(".ts") { "video/mp2t" }
    else if clean.ends_with(".mp4") { "video/mp4" }
    else if clean.ends_with(".vtt") { "text/vtt; charset=utf-8" }
    else { "application/octet-stream" }
}

fn text_response(status: u16, msg: &str) -> Response<BoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .header("Access-Control-Allow-Origin", "*")
        .body(Full::new(Bytes::from(msg.to_string())).map_err(|e| match e {}).boxed())
        .unwrap()
}

fn encode_b64(url: &str) -> String { URL_SAFE_NO_PAD.encode(url.as_bytes()) }
fn decode_b64(token: &str) -> Option<String> { URL_SAFE_NO_PAD.decode(token.as_bytes()).ok().and_then(|b| String::from_utf8(b).ok()) }
