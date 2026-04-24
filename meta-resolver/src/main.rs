mod models;

use models::{
    BatchResolveRequest, JsonRpcRequest, JsonRpcResponse, ResolveIdRequest, ResolveIdResult,
    ResolveStreamRequest, ResolveStreamResult,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::io::{self, BufRead};
use futures_util::StreamExt;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

#[tokio::main]
async fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let client = Client::builder()
        .user_agent(UA)
        .build()
        .unwrap();

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
                    send_response(req.id, json!({"ok": true, "version": "1.0.0"}));
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

async fn resolve_stream_logic(client: &Client, params: ResolveStreamRequest) -> ResolveStreamResult {
    let mut result = ResolveStreamResult::default();
    
    // Reference fields to satisfy compiler/allow future episode-specific logic
    let _ = params.season;
    let _ = params.episode;

    // 1. Map ID first
    let imdb_id = match fetch_imdb_id(client, &params.tmdb_id.to_string(), &params.media_type).await {
        Ok(id) => id,
        Err(e) => {
            result.error_code = Some("ID_MAP_FAIL".to_string());
            result.error_message = Some(e);
            return result;
        }
    };

    // 2. Fetch direct signed URLs
    match fetch_signed_streams(client, &imdb_id).await {
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
            tokio::spawn(async move {
                resolve_stream_logic(&client, p).await
            })
        })
        .buffer_unordered(workers)
        .collect::<Vec<_>>()
        .await;

    results.into_iter().map(|r| r.unwrap()).collect()
}

async fn fetch_imdb_id(client: &Client, tid: &str, mtype: &str) -> Result<String, String> {
    let url = format!("https://db.videasy.net/3/{}/{}?append_to_response=external_ids", mtype, tid);
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(id) = json["external_ids"]["imdb_id"].as_str() {
            return Ok(id.to_string());
        }
    }
    Err("IMDb ID not found".to_string())
}

async fn fetch_signed_streams(client: &Client, iid: &str) -> Result<String, String> {
    let url = format!("https://trailers.videasy.net/getOldestTrailer?id={}", iid);
    let resp = client.get(url)
        .header("Origin", "https://www.cineby.sc")
        .header("Referer", "https://www.cineby.sc/")
        .send().await.map_err(|e| e.to_string())?;
    
    if resp.status() == 200 {
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(streams) = json["trailer"]["streams"].as_array() {
            // Pick 1080p if available, else first
            let best = streams.iter()
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
