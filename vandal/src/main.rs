mod crypto;
mod models;
mod proxy;
mod videasy;
mod wasm;

use models::{JsonRpcRequest, JsonRpcResponse, JsonRpcError, ResolveStreamParams, StreamResult};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let wasm_bytes = std::fs::read(r"E:\fromDesktop\moviebox\videasy_api\videasy_wasm.wasm")
        .expect("Failed to read videasy_wasm.wasm. Ensure it is in the correct directory.");
    
    let decryptor = Arc::new(wasm::WasmDecryptor::new(&wasm_bytes).expect("Failed to initialize WASM decryptor"));
    let client = reqwest::Client::new();
    let proxy_port = proxy::ensure_proxy_running().await;

    let stdin = io::stdin();
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
            Err(e) => {
                eprintln!("[Vandal] Failed to parse RPC request: {}", e);
                continue;
            }
        };

        let req_id = req.id.clone();
        
        if req.method == "resolveStream" {
            let params: ResolveStreamParams = match serde_json::from_value(req.params) {
                Ok(p) => p,
                Err(e) => {
                    let err = JsonRpcError { code: -32602, message: format!("Invalid params: {}", e) };
                    let res: JsonRpcResponse<StreamResult> = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        protocol_version: "1.0".to_string(),
                        id: req_id,
                        result: None,
                        error: Some(err),
                    };
                    if let Ok(json) = serde_json::to_string(&res) {
                        let _ = writeln!(io::stdout(), "{}", json);
                        let _ = io::stdout().flush();
                    }
                    continue;
                }
            };

            let decryptor_cl = Arc::clone(&decryptor);
            let client_cl = client.clone();
            
            let mut result = match videasy::resolve_videasy(
                &client_cl,
                &decryptor_cl,
                params.tmdb_id,
                &params.media_type,
                params.season,
                params.episode,
            ).await {
                Ok(res) => res,
                Err(e) => {
                    eprintln!("[Vandal] Internal error: {}", e);
                    StreamResult::error("INTERNAL_ERROR", "Extraction failed")
                }
            };

            if result.success && result.audios.is_some() {
                let audios_orig = result.audios.clone().unwrap();
                let headers_map = match result.headers.clone() {
                    Some(h) => {
                        let mut m = std::collections::HashMap::new();
                        if let Some(r) = h.referer { m.insert("Referer".to_string(), r); }
                        if let Some(o) = h.origin { m.insert("Origin".to_string(), o); }
                        if let Some(u) = h.user_agent { m.insert("User-Agent".to_string(), u); }
                        m
                    },
                    None => std::collections::HashMap::new(),
                };
                
                let session_id = proxy::create_session(&audios_orig, &headers_map).await;
                let base = format!("http://127.0.0.1:{}", proxy_port);
                
                // Rewrite audios to use proxy
                let mut proxied_audios = std::collections::HashMap::new();
                let mut best_proxied_url = None;

                for (audio_name, qualities) in audios_orig {
                    let mut p_qualities = std::collections::HashMap::new();
                    for (quality, _) in qualities {
                        let p_url = format!("{}/s/{}/{}/{}", base, session_id, urlencoding::encode(&audio_name), urlencoding::encode(&quality));
                        p_qualities.insert(quality.clone(), p_url.clone());
                        
                        if best_proxied_url.is_none() || (quality.contains("1080p") && best_proxied_url.as_ref().map_or(true, |u: &String| !u.contains("1080p"))) {
                            best_proxied_url = Some(p_url);
                        }
                    }
                    proxied_audios.insert(audio_name, p_qualities);
                }
                
                result.audios = Some(proxied_audios);
                result.stream_url = best_proxied_url;
                result.proxy_port = Some(proxy_port);
                result.session_id = Some(session_id);
                result.self_proxy = Some(true);
                result.headers = Some(models::StreamHeaders {
                    referer: None,
                    origin: None,
                    user_agent: None,
                });
                
                // Subtitles are left untouched (not proxied)
            }

            let response: JsonRpcResponse<StreamResult> = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                id: req_id,
                result: Some(result),
                error: None,
            };

            if let Ok(json) = serde_json::to_string(&response) {
                let _ = writeln!(io::stdout(), "{}", json);
                let _ = io::stdout().flush();
            }
            
        } else if req.method == "healthCheck" {
            let res = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "protocolVersion": "1.0",
                "result": {"ok": true, "version": "1.0.0"}
            });
            let _ = writeln!(io::stdout(), "{}", res);
            let _ = io::stdout().flush();
        } else {
            let err = JsonRpcError { code: -32601, message: "Method not found".to_string() };
            let res: JsonRpcResponse<()> = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                protocol_version: "1.0".to_string(),
                id: req_id,
                result: None,
                error: Some(err),
            };
            if let Ok(json) = serde_json::to_string(&res) {
                let _ = writeln!(io::stdout(), "{}", json);
                let _ = io::stdout().flush();
            }
        }
    }
}
