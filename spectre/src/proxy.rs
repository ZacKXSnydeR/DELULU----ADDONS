use warp::Filter;
use base64::{engine::general_purpose, Engine as _};
use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, REFERER, ORIGIN, USER_AGENT, RANGE, CONNECTION, ACCEPT, ACCEPT_LANGUAGE, ACCEPT_ENCODING};
use regex::Regex;
use std::sync::Arc;
use tokio::sync::RwLock;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::collections::HashSet;

#[derive(Clone)]
struct CachedSegment {
    content_type: String,
    body: Arc<Vec<u8>>,
}

pub struct Proxy {
    client: reqwest::Client,
    cache: Arc<RwLock<LruCache<String, CachedSegment>>>,
    // Registry of segments seen in m3u8 for prefetching
    segment_registry: Arc<RwLock<Vec<String>>>,
    prefetching: Arc<RwLock<HashSet<String>>>,
}

impl Proxy {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .pool_max_idle_per_host(10)
                .build()
                .unwrap(),
            cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(100).unwrap()))),
            segment_registry: Arc::new(RwLock::new(Vec::new())),
            prefetching: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    fn get_headers(&self, is_segment: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"));
        h.insert(ACCEPT, HeaderValue::from_static("*/*"));
        h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        h.insert(REFERER, HeaderValue::from_static("https://cloudnestra.com/"));
        h.insert(ORIGIN, HeaderValue::from_static("https://cloudnestra.com"));
        h.insert(CONNECTION, HeaderValue::from_static("keep-alive"));
        
        if is_segment {
            h.insert(RANGE, HeaderValue::from_static("bytes=0-"));
        }
        h
    }

    async fn fetch_origin(&self, url: &str, is_segment: bool) -> Result<(Vec<u8>, String)> {
        let headers = self.get_headers(is_segment);
        for attempt in 1..=3 {
            match self.client.get(url).headers(headers.clone()).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let ct = resp.headers().get("content-type")
                            .map(|v| v.to_str().unwrap_or("video/mp2t"))
                            .unwrap_or("video/mp2t").to_string();
                        let bytes = resp.bytes().await?.to_vec();
                        return Ok((bytes, ct));
                    }
                    if attempt < 3 { tokio::time::sleep(std::time::Duration::from_millis(500)).await; }
                }
                Err(e) => {
                    if attempt == 3 { return Err(e.into()); }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
        anyhow::bail!("Max retries exceeded")
    }

    pub fn rewrite_m3u8(&self, body: &str, base_url: &str, proxy_base: &str) -> (String, Vec<String>) {
        let lines = body.lines();
        let mut out = Vec::new();
        let mut segments = Vec::new();
        let uri_re = Regex::new(r#"URI=["']([^"']+)["']"#).unwrap();

        for line in lines {
            let mut rewritten_line = line.to_string();
            
            // Rewrite URI attributes
            if let Some(cap) = uri_re.captures(line) {
                let orig = cap.get(1).unwrap().as_str();
                let abs_url = self.join_url(base_url, orig);
                let token = general_purpose::URL_SAFE_NO_PAD.encode(&abs_url);
                rewritten_line = line.replace(orig, &format!("{}/seg/{}", proxy_base, token));
            }

            if !line.starts_with("#") && !line.trim().is_empty() {
                let abs_url = self.join_url(base_url, line);
                segments.push(abs_url.clone());
                let token = general_purpose::URL_SAFE_NO_PAD.encode(&abs_url);
                if line.contains(".m3u8") {
                    out.push(format!("{}/pl/{}", proxy_base, token));
                } else {
                    out.push(format!("{}/seg/{}", proxy_base, token));
                }
            } else {
                out.push(rewritten_line);
            }
        }
        (out.join("\n"), segments)
    }

    fn join_url(&self, base: &str, path: &str) -> String {
        if path.starts_with("http") { return path.to_string(); }
        match url::Url::parse(base) {
            Ok(base_url) => {
                match base_url.join(path) {
                    Ok(joined) => joined.to_string(),
                    Err(_) => path.to_string(),
                }
            },
            Err(_) => path.to_string(),
        }
    }

    async fn trigger_prefetch(&self, segment_url: &str) {
        let registry = self.segment_registry.read().await;
        if let Some(pos) = registry.iter().position(|s| s == segment_url) {
            let next_segments = registry.iter().skip(pos + 1).take(5).cloned().collect::<Vec<_>>();
            drop(registry);

            for url in next_segments {
                let cache = self.cache.read().await;
                if cache.contains(&url) { continue; }
                drop(cache);

                let mut prefetching = self.prefetching.write().await;
                if prefetching.contains(&url) { continue; }
                prefetching.insert(url.clone());
                drop(prefetching);

                let client = self.client.clone();
                let cache_arc = Arc::clone(&self.cache);
                let prefetching_arc = Arc::clone(&self.prefetching);
                let headers = self.get_headers(true);

                tokio::spawn(async move {
                    if let Ok(resp) = client.get(&url).headers(headers).send().await {
                        if resp.status().is_success() {
                            let ct = resp.headers().get("content-type")
                                .map(|v| v.to_str().unwrap_or("video/mp2t"))
                                .unwrap_or("video/mp2t").to_string();
                            if let Ok(bytes) = resp.bytes().await {
                                let mut cache = cache_arc.write().await;
                                cache.push(url.clone(), CachedSegment {
                                    content_type: ct,
                                    body: Arc::new(bytes.to_vec()),
                                });
                            }
                        }
                    }
                    let mut prefetching = prefetching_arc.write().await;
                    prefetching.remove(&url);
                });
            }
        }
    }
}

pub async fn run_proxy(port: u16) {
    let proxy = Arc::new(Proxy::new());
    let proxy_filter = warp::any().map(move || Arc::clone(&proxy));

    // /proxy/pl/<token>
    let pl_route = warp::path!("proxy" / "pl" / String)
        .and(proxy_filter.clone())
        .and(warp::header::optional::<String>("host"))
        .and_then(move |token, proxy: Arc<Proxy>, host: Option<String>| async move {
            let url = match general_purpose::URL_SAFE_NO_PAD.decode(token) {
                Ok(u) => String::from_utf8_lossy(&u).to_string(),
                Err(_) => return Err(warp::reject::not_found()),
            };
            
            let (body_bytes, _ct) = match proxy.fetch_origin(&url, false).await {
                Ok(r) => r,
                Err(_) => return Err(warp::reject::not_found()),
            };

            let body_str = String::from_utf8_lossy(&body_bytes);
            let host_str = host.unwrap_or_else(|| format!("localhost:{}", port));
            let proxy_base = format!("http://{}/proxy", host_str);
            let (rewritten, segments) = proxy.rewrite_m3u8(&body_str, &url, &proxy_base);

            // Update segment registry for prefetching
            let mut registry = proxy.segment_registry.write().await;
            *registry = segments;

            Ok::<_, warp::Rejection>(warp::reply::with_header(
                rewritten,
                "Content-Type",
                "application/vnd.apple.mpegurl",
            ))
        });

    // /proxy/seg/<token>
    let seg_route = warp::path!("proxy" / "seg" / String)
        .and(proxy_filter.clone())
        .and_then(|token, proxy: Arc<Proxy>| async move {
            let url = match general_purpose::URL_SAFE_NO_PAD.decode(token) {
                Ok(u) => String::from_utf8_lossy(&u).to_string(),
                Err(_) => return Err(warp::reject::not_found()),
            };

            // 1. Check Cache
            {
                let mut cache = proxy.cache.write().await;
                if let Some(cached) = cache.get(&url) {
                    let body = (*cached.body).clone();
                    let ct = cached.content_type.clone();
                    
                    let p = Arc::clone(&proxy);
                    let u = url.clone();
                    tokio::spawn(async move { p.trigger_prefetch(&u).await; });

                    return Ok::<_, warp::Rejection>(warp::reply::with_header(
                        warp::reply::Response::new(body.into()),
                        "Content-Type",
                        ct,
                    ));
                }
            }

            // 2. Cache Miss: Fetch and Store
            let (bytes, ct) = match proxy.fetch_origin(&url, true).await {
                Ok(r) => r,
                Err(_) => return Err(warp::reject::not_found()),
            };

            {
                let mut cache = proxy.cache.write().await;
                cache.push(url.clone(), CachedSegment {
                    content_type: ct.clone(),
                    body: Arc::new(bytes.clone()),
                });
            }

            let p = Arc::clone(&proxy);
            let u = url.clone();
            tokio::spawn(async move { p.trigger_prefetch(&u).await; });

            Ok::<_, warp::Rejection>(warp::reply::with_header(
                warp::reply::Response::new(bytes.into()),
                "Content-Type",
                ct,
            ))
        });

    warp::serve(pl_route.or(seg_route))
        .run(([0, 0, 0, 0], port))
        .await;
}
