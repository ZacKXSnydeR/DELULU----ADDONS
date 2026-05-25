use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rquest::Client;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::OnceCell;

const REFERER: &str = "https://vidlink.pro/";
const ORIGIN: &str = "https://vidlink.pro";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) \
    Chrome/124.0.0.0 Safari/537.36";

struct ProxyState {
    port: u16,
}

static PROXY: OnceCell<ProxyState> = OnceCell::const_new();

pub async fn ensure_proxy_running() -> u16 {
    let state = PROXY
        .get_or_init(|| async {
            let addr = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = TcpListener::bind(addr)
                .await
                .expect("[proxy] bind failed");
            let port = listener.local_addr().unwrap().port();

            let client = Arc::new(
                Client::builder()
                    .gzip(true)
                    .brotli(true)
                    .timeout(std::time::Duration::from_secs(60))
                    .pool_max_idle_per_host(12)
                    .build()
                    .unwrap_or_else(|_| Client::new()),
            );

            tokio::spawn(async move {
                eprintln!("[proxy] listening on http://127.0.0.1:{}", port);
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let io = TokioIo::new(stream);
                            let c = client.clone();
                            tokio::spawn(async move {
                                let svc = service_fn(move |req| {
                                    let c = c.clone();
                                    async move { handle(req, c, port).await }
                                });
                                let _ = http1::Builder::new()
                                    .keep_alive(true)
                                    .serve_connection(io, svc)
                                    .await;
                            });
                        }
                        Err(e) => eprintln!("[proxy] accept error: {e}"),
                    }
                }
            });

            ProxyState { port }
        })
        .await;

    state.port
}

fn cdn_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Referer", REFERER),
        ("Origin", ORIGIN),
        ("User-Agent", UA),
        ("Accept", "*/*"),
        ("Accept-Language", "en-US,en;q=0.9"),
        ("DNT", "1"),
        ("Connection", "keep-alive"),
        ("Sec-Fetch-Dest", "empty"),
        ("Sec-Fetch-Mode", "cors"),
        ("Sec-Fetch-Site", "cross-site"),
        ("Sec-CH-UA", r#""Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99""#),
        ("Sec-CH-UA-Mobile", "?0"),
        ("Sec-CH-UA-Platform", r#""Windows""#),
    ]
}

fn encode_b64(url: &str) -> String {
    URL_SAFE_NO_PAD.encode(url.as_bytes())
}

fn decode_b64(token: &str) -> Option<String> {
    URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
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

fn content_type_for_url(url: &str) -> &'static str {
    let clean = url.split('?').next().unwrap_or(url).to_lowercase();
    if clean.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if clean.ends_with(".ts") {
        "video/mp2t"
    } else if clean.ends_with(".mp4") || clean.ends_with(".m4v") || clean.ends_with(".m4s") {
        "video/mp4"
    } else if clean.ends_with(".vtt") {
        "text/vtt; charset=utf-8"
    } else if clean.ends_with(".srt") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn rewrite_playlist(body: &str, base_url: &str, proxy_base: &str) -> String {
    let mut out = String::with_capacity(body.len() * 2);
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        let mut rewritten = line.to_string();
        if rewritten.contains("URI=\"") {
            let mut new_line = String::new();
            let mut rest = rewritten.as_str();
            while let Some(start) = rest.find("URI=\"") {
                new_line.push_str(&rest[..start]);
                let attr_start = start + 5;
                if let Some(end) = rest[attr_start..].find('"') {
                    let uri = rest[attr_start..attr_start + end].to_string();
                    let abs = resolve_url(base_url, &uri);
                    let token = encode_b64(&abs);
                    new_line.push_str(&format!("URI=\"{}/b64/{}\"", proxy_base, token));
                    rest = &rest[attr_start + end + 1..];
                } else {
                    new_line.push_str(&rest[start..]);
                    rest = "";
                    break;
                }
            }
            new_line.push_str(rest);
            rewritten = new_line;
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

async fn proxy_upstream(
    client: &Client,
    upstream_url: &str,
    incoming: &Request<hyper::body::Incoming>,
    proxy_base: &str,
) -> Response<BoxBody<Bytes, Infallible>> {
    let is_playlist = upstream_url
        .split('?')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .ends_with(".m3u8");

    let mut req = client.get(upstream_url);
    for (k, v) in cdn_headers() {
        req = req.header(k, v);
    }
    if let Some(range) = incoming.headers().get("range") {
        if let Ok(s) = range.to_str() {
            req = req.header("Range", s);
        }
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[proxy] upstream error: {e}");
            return text_response(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}"));
        }
    };

    let status = resp.status();
    let hyper_status = if status == rquest::StatusCode::PARTIAL_CONTENT {
        StatusCode::PARTIAL_CONTENT
    } else if status.is_success() {
        StatusCode::OK
    } else {
        eprintln!("[proxy] upstream {} → {}", status.as_u16(), &upstream_url[..upstream_url.len().min(80)]);
        return text_response(
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            &format!("upstream returned {status}"),
        );
    };

    let content_length = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).map(String::from);
    let content_range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).map(String::from);
    let accept_ranges = resp.headers().get("accept-ranges").and_then(|v| v.to_str().ok()).map(String::from);
    let ct = content_type_for_url(upstream_url);

    if is_playlist {
        let body_bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return text_response(StatusCode::BAD_GATEWAY, &format!("read error: {e}")),
        };
        let body_str = String::from_utf8_lossy(&body_bytes);
        let rewritten = rewrite_playlist(&body_str, upstream_url, proxy_base);
        return Response::builder()
            .status(200)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Content-Length", rewritten.len().to_string())
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "no-cache")
            .body(full_body(Bytes::from(rewritten)))
            .unwrap();
    }

    let mut builder = Response::builder()
        .status(hyper_status)
        .header("Content-Type", ct)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "Content-Length, Content-Range, Accept-Ranges")
        .header("Cache-Control", "public, max-age=3600");

    if let Some(cl) = content_length {
        builder = builder.header("Content-Length", cl);
    }
    if let Some(cr) = content_range {
        builder = builder.header("Content-Range", cr);
    }
    builder = builder.header("Accept-Ranges", accept_ranges.as_deref().unwrap_or("bytes"));

    let stream = resp.bytes_stream().map(|result| -> Result<Frame<Bytes>, Infallible> {
        match result {
            Ok(chunk) => Ok(Frame::data(chunk)),
            Err(_) => Ok(Frame::data(Bytes::new())),
        }
    });

    let body = StreamBody::new(stream);
    builder.body(BodyExt::boxed(body)).unwrap()
}

async fn handle(
    req: Request<hyper::body::Incoming>,
    client: Arc<Client>,
    port: u16,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    if req.method() == Method::OPTIONS {
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

    // /proxy?url=<encoded_url> — legacy compat / direct proxy
    if path == "/proxy" {
        let target = query
            .split('&')
            .find_map(|p| p.strip_prefix("url="))
            .and_then(|v| urlencoding::decode(v).ok())
            .map(|v| v.into_owned());

        return match target {
            Some(url) => Ok(proxy_upstream(&client, &url, &req, &proxy_base).await),
            None => Ok(text_response(StatusCode::BAD_REQUEST, "missing url param")),
        };
    }

    // /b64/<base64_encoded_url> — used by rewritten HLS manifests
    if let Some(token) = path.strip_prefix("/b64/") {
        return match decode_b64(token) {
            Some(url) => Ok(proxy_upstream(&client, &url, &req, &proxy_base).await),
            None => Ok(text_response(StatusCode::BAD_REQUEST, "invalid b64 token")),
        };
    }

    // /health — quick liveness check
    if path == "/health" {
        return Ok(text_response(StatusCode::OK, "ok"));
    }

    Ok(text_response(StatusCode::NOT_FOUND, "not found"))
}
