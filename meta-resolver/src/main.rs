mod models;

use futures_util::StreamExt;
use models::{
    BatchResolveRequest, JsonRpcRequest, JsonRpcResponse, ResolveIdRequest, ResolveIdResult,
    ResolveStreamRequest, ResolveStreamResult,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::io::{self, BufRead};
use regex::Regex;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

const SUPABASE_URL: &str = "https://sjkhmshfcoadmcufukpb.supabase.co";
const SUPABASE_ANON_KEY: &str = env!("SUPABASE_ANON_KEY");

const GRAPHQL_ENDPOINT: &str = "https://graphql.imdb.com/";
const QUERY_TITLE_VIDEOS: &str = "query TitleVideoGallerySubPage($const: ID!, $first: Int!, $filter: VideosQueryFilter, $sort: VideoSort) { title(id: $const) { videoStrip(first: $first, filter: $filter, sort: $sort) { edges { node { id name { value } } } } } }";
const QUERY_VIDEO_PLAYBACK: &str = "query VideoPlayback($id: ID!) { video(id: $id) { playbackURLs { url videoMimeType videoDefinition } } }";

#[tokio::main]
async fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let client = Client::builder().user_agent(UA).build().unwrap();

    let mut line = String::new();
    while handle.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
            // Verify protocol version to use the field
            if req.jsonrpc != "2.0" {
                line.clear();
                continue;
            }

            match req.method.as_str() {
                // Use Case 1: ID Mapping (Middleware)
                "resolveId" => {
                    if let Ok(params) = serde_json::from_value::<ResolveIdRequest>(req.params) {
                        let result = resolve_id_logic(&client, params).await;
                        send_response(req.id, result);
                    }
                }
                // Use Case 2: Trailer/Stream Resolver
                "resolveStream" => {
                    if let Ok(params) = serde_json::from_value::<ResolveStreamRequest>(req.params) {
                        let result = resolve_stream_logic(&client, params).await;
                        send_response(req.id, result);
                    }
                }
                // Parallel Bulk Processing
                "batchResolve" => {
                    if let Ok(params) = serde_json::from_value::<BatchResolveRequest>(req.params) {
                        let results = resolve_batch(&client, params).await;
                        send_response(req.id, results);
                    }
                }
                "healthCheck" => {
                    send_response(
                        req.id,
                        json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
                    );
                }
                _ => {}
            }
        }
        line.clear();
    }

    Ok(())
}

async fn resolve_id_logic(client: &Client, params: ResolveIdRequest) -> ResolveIdResult {
    let mut result = ResolveIdResult {
        tmdb_id: params.tmdb_id,
        ..Default::default()
    };
    match fetch_imdb_id(client, &params.tmdb_id.to_string(), &params.media_type).await {
        Ok(id) => {
            result.success = true;
            result.imdb_id = Some(id);
        }
        Err(e) => result.error = Some(e),
    }
    result
}

async fn resolve_stream_logic(
    client: &Client,
    params: ResolveStreamRequest,
) -> ResolveStreamResult {
    let mut result = ResolveStreamResult::default();

    // Reference episode field for future use
    let _ = params.episode;

    // 1. Map ID first
    let imdb_id = match fetch_imdb_id(client, &params.tmdb_id.to_string(), &params.media_type).await
    {
        Ok(id) => id,
        Err(e) => {
            result.error_code = Some("ID_MAP_FAIL".to_string());
            result.error_message = Some(e);
            return result;
        }
    };

    // 2. Fetch direct signed URLs with season awareness
    match fetch_streams_with_season(client, &imdb_id, params.season).await {
        Ok(url) => {
            result.success = true;
            result.stream_url = Some(url);
            // No headers returned as requested - URLs are already signed/direct
        }
        Err(e) => {
            result.error_code = Some("RESOLVE_FAIL".to_string());
            result.error_message = Some(e);
        }
    }
    result
}

async fn resolve_batch(client: &Client, params: BatchResolveRequest) -> Vec<ResolveStreamResult> {
    let workers = params.workers.unwrap_or(10);
    let results = futures_util::stream::iter(params.items)
        .map(|p| {
            let client = client.clone();
            tokio::spawn(async move { resolve_stream_logic(&client, p).await })
        })
        .buffer_unordered(workers)
        .collect::<Vec<_>>()
        .await;

    results.into_iter().map(|r| r.unwrap()).collect()
}

async fn fetch_imdb_id(client: &Client, tid: &str, mtype: &str) -> Result<String, String> {
    let url = format!(
        "{}/rest/v1/id_mappings?tmdb_id=eq.{}&media_type=eq.{}&select=imdb_id",
        SUPABASE_URL, tid, mtype
    );
    let resp = client
        .get(&url)
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", SUPABASE_ANON_KEY))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(row) = json.as_array().and_then(|a| a.first()) {
            if let Some(id) = row["imdb_id"].as_str() {
                return Ok(id.to_string());
            }
        }
    }
    Err("IMDb ID not found in Supabase".to_string())
}

async fn fetch_streams_with_season(
    client: &Client,
    iid: &str,
    season: Option<u32>,
) -> Result<String, String> {
    // Primary: IMDb GraphQL (cookie-free, no external dependencies)
    if let Ok(url) = fetch_trailer_via_graphql(client, iid, season).await {
        return Ok(url);
    }
    // Fallback: IMDb HTML scraping
    fetch_trailer_via_html(client, iid).await
}

async fn fetch_trailer_via_graphql(
    client: &Client,
    iid: &str,
    season: Option<u32>,
) -> Result<String, String> {
    let payload = json!({
        "operationName": "TitleVideoGallerySubPage",
        "query": QUERY_TITLE_VIDEOS,
        "variables": {
            "const": iid,
            "first": 20,
            "filter": {
                "maturityLevel": "INCLUDE_MATURE",
                "nameConstraints": {},
                "titleConstraints": {},
                "types": ["TRAILER"]
            },
            "sort": { "by": "DATE", "order": "DESC" }
        }
    });

    let resp = client
        .post(GRAPHQL_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("Referer", "https://www.imdb.com/")
        .header("Origin", "https://www.imdb.com")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("GraphQL title query failed: {}", resp.status()));
    }

    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    let edges = json
        .pointer("/data/title/videoStrip/edges")
        .and_then(|e| e.as_array())
        .ok_or("No trailers in IMDb response")?;

    if edges.is_empty() {
        return Err("No trailers found for this title".to_string());
    }

    // If season specified, prefer a trailer whose name matches the season pattern;
    // fall back to the newest (first, sorted date desc) if no match.
    let video_id = if let Some(season_num) = season {
        edges
            .iter()
            .find(|edge| {
                edge.pointer("/node/name/value")
                    .and_then(|n| n.as_str())
                    .map(|name| matches_season_pattern(name, season_num))
                    .unwrap_or(false)
            })
            .or_else(|| edges.first())
    } else {
        edges.first()
    }
    .and_then(|e| e.pointer("/node/id"))
    .and_then(|id| id.as_str())
    .ok_or("No valid video ID in IMDb response")?;

    fetch_playback_url(client, video_id).await
}

fn matches_season_pattern(name: &str, season: u32) -> bool {
    let lower = name.to_lowercase();
    let season_digits = season.to_string();

    // Pattern 1: "season N"
    let season_literal = format!("season {}", season_digits);
    if lower.match_indices(&season_literal).any(|(idx, _)| {
        let next_idx = idx + season_literal.len();
        lower
            .as_bytes()
            .get(next_idx)
            .map(|c| !c.is_ascii_digit())
            .unwrap_or(true)
    }) {
        return true;
    }

    // Pattern 2: tokenized Sxx forms like "S2", "S2E1", "S02"
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b's' && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 {
                let found = &lower[i + 1..j];
                if found.parse::<u32>().ok() == Some(season)
                    && bytes.get(j).map(|c| !c.is_ascii_digit()).unwrap_or(true)
                {
                    return true;
                }
            }
        }
        i += 1;
    }

    false
}

async fn fetch_playback_url(client: &Client, video_id: &str) -> Result<String, String> {
    let payload = json!({
        "operationName": "VideoPlayback",
        "query": QUERY_VIDEO_PLAYBACK,
        "variables": { "id": video_id }
    });

    let resp = client
        .post(GRAPHQL_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("Referer", "https://www.imdb.com/")
        .header("Origin", "https://www.imdb.com")
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("GraphQL playback query failed: {}", resp.status()));
    }

    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    let urls = json
        .pointer("/data/video/playbackURLs")
        .and_then(|u| u.as_array())
        .ok_or("No playbackURLs in IMDb response")?;

    // Prefer 1080p/720p MP4, then any MP4, then first entry
    let mut best: Option<String> = None;
    for entry in urls {
        let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("");
        let def = entry.get("videoDefinition").and_then(|d| d.as_str()).unwrap_or("");
        let is_mp4 = entry
            .get("videoMimeType")
            .and_then(|m| m.as_str())
            .map(|s| s.to_lowercase().contains("mp4"))
            .unwrap_or_else(|| url.contains(".mp4"));
        if is_mp4 {
            if def.contains("1080") || def.contains("720") {
                return Ok(url.to_string());
            }
            if best.is_none() {
                best = Some(url.to_string());
            }
        }
    }
    if let Some(u) = best {
        return Ok(u);
    }
    urls.first()
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str())
        .map(|u| u.to_string())
        .ok_or_else(|| "No playable stream URL found".to_string())
}

async fn fetch_trailer_via_html(client: &Client, iid: &str) -> Result<String, String> {
    // Fetch IMDb title page to extract the embedded video ID (vi...)
    let movie_url = format!("https://www.imdb.com/title/{}/", iid);
    let resp = client
        .get(&movie_url)
        .header("Referer", "https://www.imdb.com/")
        .header("Origin", "https://www.imdb.com")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("IMDb title page fetch failed: {}", resp.status()));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;

    let re_embed = Regex::new(r#""embedUrl"\s*:\s*"([^"]+)""#).unwrap();
    let re_vi = Regex::new(r"(vi\d+)").unwrap();
    let re_vi_path = Regex::new(r#"/video/(vi\d+)"#).unwrap();
    let video_id = if let Some(cap) = re_embed.captures(&body) {
        re_vi.captures(&cap[1]).map(|c| c[1].to_string())
    } else {
        re_vi_path.captures(&body).map(|c| c[1].to_string())
    }
    .ok_or("Could not find trailer video ID in IMDb HTML")?;

    // Fetch the video page to get signed playback URLs from __NEXT_DATA__
    let video_url = format!("https://www.imdb.com/video/{}/", video_id);
    let video_resp = client
        .get(&video_url)
        .header("Referer", "https://www.imdb.com/")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !video_resp.status().is_success() {
        return Err(format!("IMDb video page fetch failed: {}", video_resp.status()));
    }
    let video_body = video_resp.text().await.map_err(|e| e.to_string())?;

    let re_next = Regex::new(r#"<script[^>]*id="__NEXT_DATA__"[^>]*>(.*?)</script>"#).unwrap();
    if let Some(cap) = re_next.captures(&video_body) {
        if let Ok(val) = serde_json::from_str::<Value>(&cap[1]) {
            if let Some(urls) = val
                .pointer("/props/pageProps/videoPlaybackData/video/playbackURLs")
                .and_then(|v| v.as_array())
            {
                for entry in urls {
                    let url = entry.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let def = entry.get("videoDefinition").and_then(|d| d.as_str()).unwrap_or("");
                    if url.contains(".mp4") && (def.contains("1080") || def.contains("720")) {
                        return Ok(url.to_string());
                    }
                }
                if let Some(u) = urls.first().and_then(|e| e.get("url")).and_then(|u| u.as_str()) {
                    return Ok(u.to_string());
                }
            }
        }
    }

    // Last resort: regex scan for signed CDN MP4 URL
    let re_mp4 = Regex::new(r#"https?://[a-zA-Z0-9.-]*media-imdb\.com/[^"\s]+\.mp4\?[^"\s]+"#).unwrap();
    let unescaped = video_body.replace(r"\u002F", "/").replace(r"\/", "/");
    if let Some(cap) = re_mp4.captures(&unescaped) {
        return Ok(cap[0].to_string());
    }

    Err("HTML fallback: failed to extract MP4 URL".to_string())
}

fn send_response<T: serde::Serialize>(id: Option<Value>, result: T) {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    };
    if let Ok(json) = serde_json::to_string(&resp) {
        println!("{}", json);
    }
}
