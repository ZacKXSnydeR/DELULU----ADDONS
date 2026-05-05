mod models;

use futures_util::StreamExt;
use models::{
    BatchResolveRequest, JsonRpcRequest, JsonRpcResponse, ResolveIdRequest, ResolveIdResult,
    ResolveStreamRequest, ResolveStreamResult,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::io::{self, BufRead};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

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
        "https://db.videasy.net/3/{}/{}?append_to_response=external_ids",
        mtype, tid
    );
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(id) = json["external_ids"]["imdb_id"].as_str() {
            return Ok(id.to_string());
        }
    }
    Err("IMDb ID not found".to_string())
}

async fn fetch_streams_with_season(
    client: &Client,
    iid: &str,
    season: Option<u32>,
) -> Result<String, String> {
    // Keep legacy behavior for non-season requests to avoid regressions in existing flows.
    if season.is_none() {
        return fetch_oldest_trailer_stream(client, iid).await;
    }

    // Season-specific requests: try season-aware list first.
    if let Ok(video_id) = fetch_trailer_list_and_filter(client, iid, season).await {
        if let Ok(url) = fetch_stream_by_video_id(client, &video_id).await {
            return Ok(url);
        }
    }

    // Fallback: getOldestTrailer (most reliable baseline)
    fetch_oldest_trailer_stream(client, iid).await
}

async fn fetch_trailer_list_and_filter(
    client: &Client,
    iid: &str,
    season: Option<u32>,
) -> Result<String, String> {
    let url = format!(
        "https://trailers.videasy.net/getTrailerList?id={}&sort=date,desc&first=50&cursor=",
        iid
    );
    let resp = client
        .get(&url)
        .header("Origin", "https://www.cineby.sc")
        .header("Referer", "https://www.cineby.sc/")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() != 200 {
        return Err("getTrailerList failed".to_string());
    }

    let json: Value = resp.json().await.map_err(|e| e.to_string())?;

    // API returns { id, imdb_url, trailers: [...], count, totalCount, ... }
    let trailers = json["trailers"]
        .as_array()
        .ok_or("Invalid trailer list format")?;

    if trailers.is_empty() {
        return Err("No trailers found".to_string());
    }

    // If season specified, filter for season-matching trailers
    if let Some(season_num) = season {
        let filtered: Vec<_> = trailers
            .iter()
            .filter(|t| {
                if let Some(name) = t["name"].as_str() {
                    matches_season_pattern(name, season_num)
                } else {
                    false
                }
            })
            .collect();

        if !filtered.is_empty() {
            // Return first match (list is already sorted by date desc, so newest first)
            if let Some(id) = filtered[0]["id"].as_str() {
                return Ok(id.to_string());
            }
        }
        // No season match found, fall through to any trailer
    }

    // Return first trailer (newest, due to sort=date,desc)
    if let Some(id) = trailers[0]["id"].as_str() {
        Ok(id.to_string())
    } else {
        Err("No valid trailer ID found".to_string())
    }
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

async fn fetch_stream_by_video_id(client: &Client, vid: &str) -> Result<String, String> {
    let url = format!("https://trailers.videasy.net/getStream?id={}", vid);
    let resp = client
        .get(&url)
        .header("Origin", "https://www.cineby.sc")
        .header("Referer", "https://www.cineby.sc/")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() != 200 {
        return Err("getStream failed".to_string());
    }

    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(streams) = json["streams"].as_array() {
        // Pick 1080p if available, else first
        let best = streams
            .iter()
            .find(|s| s["quality"].as_str().unwrap_or("").contains("1080p"))
            .or_else(|| streams.first());

        if let Some(u) = best.and_then(|s| s["url"].as_str()) {
            return Ok(u.to_string());
        }
    }
    Err("No stream URL found".to_string())
}

async fn fetch_oldest_trailer_stream(client: &Client, iid: &str) -> Result<String, String> {
    let url = format!("https://trailers.videasy.net/getOldestTrailer?id={}", iid);
    let resp = client
        .get(url)
        .header("Origin", "https://www.cineby.sc")
        .header("Referer", "https://www.cineby.sc/")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 {
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(streams) = json["trailer"]["streams"].as_array() {
            // Pick 1080p if available, else first
            let best = streams
                .iter()
                .find(|s| s["quality"].as_str().unwrap_or("").contains("1080p"))
                .or_else(|| streams.first());

            if let Some(u) = best.and_then(|s| s["url"].as_str()) {
                return Ok(u.to_string());
            }
        }
    }
    Err("No direct stream found".to_string())
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
