use serde_json::json;
use std::io::{self, BufRead, Write};

mod models;
mod http;
mod tmdb;
mod flixhq;
mod rabbitstream;

use models::{JsonRpcRequest, JsonRpcResponse, ResolveStreamParams, StreamResult};

#[tokio::main]
async fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    let client = http::build_client();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => {
                let res: JsonRpcResponse<serde_json::Value> = JsonRpcResponse {
                    id: None,
                    jsonrpc: "2.0".to_string(),
                    protocol_version: "1.0".to_string(),
                    result: None,
                    error: Some(models::JsonRpcError {
                        code: -32700,
                        message: "Parse error".to_string(),
                    }),
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&res).unwrap());
                let _ = stdout.flush();
                continue;
            }
        };

        match req.method.as_str() {
            "healthCheck" => {
                let res = JsonRpcResponse {
                    id: req.id.clone(),
                    jsonrpc: "2.0".to_string(),
                    protocol_version: "1.0".to_string(),
                    result: Some(json!({
                        "ok": true,
                        "version": "1.0.0",
                        "latencyMs": 0
                    })),
                    error: None,
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&res).unwrap());
            }
            "resolveStream" => {
                let res = handle_resolve_stream(&client, &req).await;
                let _ = writeln!(stdout, "{}", serde_json::to_string(&res).unwrap());
            }
            _ => {
                let res: JsonRpcResponse<serde_json::Value> = JsonRpcResponse {
                    id: req.id.clone(),
                    jsonrpc: "2.0".to_string(),
                    protocol_version: "1.0".to_string(),
                    result: None,
                    error: Some(models::JsonRpcError {
                        code: -32601,
                        message: "Method not found".to_string(),
                    }),
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&res).unwrap());
            }
        }
        let _ = stdout.flush();
    }
}

async fn handle_resolve_stream(client: &reqwest::Client, req: &JsonRpcRequest) -> JsonRpcResponse<StreamResult> {
    let params: ResolveStreamParams = match serde_json::from_value(req.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(StreamResult::error("INVALID_PARAMS", &e.to_string())),
                error: None,
            };
        }
    };

    // TMDB API Key is injected via environment variable by Delulu runtime
    let tmdb_api_key = match std::env::var("TMDB_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => {
            // Fallback: check params (for standalone testing)
            match params.tmdb_api_key {
                Some(key) if !key.trim().is_empty() => key,
                _ => {
                    return JsonRpcResponse {
                        id: req.id.clone(),
                        jsonrpc: "2.0".to_string(),
                        protocol_version: "1.0".to_string(),
                        result: Some(StreamResult::error("MISSING_API_KEY", "TMDB API key is required but was not provided")),
                        error: None,
                    };
                }
            }
        }
    };

    let tmdb_info = match tmdb::fetch_tmdb_info(
        client,
        params.tmdb_id,
        &params.media_type,
        params.season,
        params.episode,
        &tmdb_api_key,
    ).await {
        Ok(info) => info,
        Err(e) => {
            return JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(StreamResult::error("TMDB_ERROR", &e.to_string())),
                error: None,
            };
        }
    };

    let search_results = match flixhq::search_flixhq(client, &tmdb_info.title).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(StreamResult::error("SEARCH_ERROR", &e.to_string())),
                error: None,
            };
        }
    };

    let scored = flixhq::score_results(&tmdb_info, search_results);
    if scored.is_empty() {
        return JsonRpcResponse {
            id: req.id.clone(),
            jsonrpc: "2.0".to_string(),
            protocol_version: "1.0".to_string(),
            result: Some(StreamResult::error("NO_MATCH", "No results passed validation")),
            error: None,
        };
    }

    let mut watch_url = scored[0].link.clone();

    if tmdb_info.media_type == "tv" {
        let season = tmdb_info.season.unwrap_or(1);
        let episode = tmdb_info.episode.unwrap_or(1);
        match flixhq::extract_episode_link(client, &watch_url, season, episode).await {
            Ok(Some(link)) => watch_url = link,
            Ok(None) => {
                return JsonRpcResponse {
                    id: req.id.clone(),
                    jsonrpc: "2.0".to_string(),
                    protocol_version: "1.0".to_string(),
                    result: Some(StreamResult::error("EPISODE_NOT_FOUND", "Could not locate episode link")),
                    error: None,
                };
            }
            Err(e) => {
                return JsonRpcResponse {
                    id: req.id.clone(),
                    jsonrpc: "2.0".to_string(),
                    protocol_version: "1.0".to_string(),
                    result: Some(StreamResult::error("EPISODE_ERROR", &e.to_string())),
                    error: None,
                };
            }
        }
    }

    let servers = match flixhq::get_servers(client, &watch_url, tmdb_info.media_type == "tv").await {
        Ok(s) => s,
        Err(e) => {
            return JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(StreamResult::error("SERVERS_ERROR", &e.to_string())),
                error: None,
            };
        }
    };

    let mut target_embed = None;
    for srv in servers {
        if let Some(name) = srv["name"].as_str() {
            let n = name.to_lowercase();
            if !n.contains("videasy") && !n.contains("vidking") {
                target_embed = srv["link"].as_str().map(|s| s.to_string());
                break;
            }
        }
    }

    if let Some(embed) = target_embed {
        match rabbitstream::extract(client, &embed).await {
            Ok(res) => JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(res),
                error: None,
            },
            Err(e) => JsonRpcResponse {
                id: req.id.clone(),
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                result: Some(StreamResult::error("EXTRACTION_ERROR", &e.to_string())),
                error: None,
            }
        }
    } else {
        JsonRpcResponse {
            id: req.id.clone(),
            jsonrpc: "2.0".to_string(),
            protocol_version: "1.0".to_string(),
            result: Some(StreamResult::error("NO_SERVERS", "Could not find target server")),
            error: None,
        }
    }
}
