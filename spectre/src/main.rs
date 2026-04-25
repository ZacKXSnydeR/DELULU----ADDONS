mod models;
mod resolver;
mod proxy;

use models::*;
use resolver::Resolver;
use std::io::{self, BufRead};
use std::sync::Arc;
use std::collections::HashMap;
use base64::{engine::general_purpose, Engine as _};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let resolver = Arc::new(Resolver::new());
    let port = 8899; 
    let session_id = format!("spectre-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
    
    tokio::spawn(async move {
        proxy::run_proxy(port).await;
    });

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() { continue; }

        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let response = match req.method.as_str() {
            "resolveStream" => {
                let params: ResolveParams = serde_json::from_value(req.params).unwrap();
                let tmdb_id = match &params.tmdb_id {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => "".to_string(),
                };
                
                let season = params.season.unwrap_or(1);
                let episode = params.episode.unwrap_or(1);
                
                // 1. Parallel Fetching
                let res_to = Arc::clone(&resolver);
                let res_me = Arc::clone(&resolver);
                let m_type_to = params.media_type.clone();
                let m_type_me = params.media_type.clone();
                let id_to = tmdb_id.clone();
                let id_me = tmdb_id.clone();
                
                let to_handle = tokio::spawn(async move { res_to.resolve_vidsrc_to(&id_to, &m_type_to, season, episode).await });
                let me_handle = tokio::spawn(async move { res_me.resolve_vidsrc_me(&id_me, &m_type_me, season, episode).await });

                // 2. Collection list for mirrors
                let mut all_mirrors: Vec<String> = Vec::new();
                
                if let Ok(Ok(links)) = to_handle.await {
                    all_mirrors.extend(links);
                }
                if let Ok(Ok(links)) = me_handle.await {
                    all_mirrors.extend(links);
                }

                if !all_mirrors.is_empty() {
                    let mut server_list = HashMap::new();
                    let mut primary_url = None;

                    for (i, url) in all_mirrors.iter().enumerate() {
                        let token = general_purpose::URL_SAFE_NO_PAD.encode(url);
                        let proxy_url = format!("http://localhost:{}/proxy/pl/{}", port, token);
                        
                        let label = format!("Server {}", i + 1);
                        server_list.insert(label.clone(), proxy_url.clone());
                        
                        if i == 0 { primary_url = Some(proxy_url); }
                    }

                    // Map all servers under a single "Original Audio" group
                    let mut audios = HashMap::new();
                    audios.insert("Original Audio".to_string(), server_list);

                    let result = StreamResult {
                        success: true,
                        stream_url: primary_url,
                        headers: Some(HashMap::from([
                            ("Referer".to_string(), "https://cloudnestra.com/".to_string()),
                            ("Origin".to_string(), "https://cloudnestra.com".to_string()),
                        ])),
                        subtitles: vec![],
                        audios,
                        error_code: None,
                        error_message: None,
                        provider: Some("spectre".to_string()),
                        proxy_port: Some(port),
                        self_proxy: true,
                        session_id: session_id.clone(),
                    };
                    RpcResponse { jsonrpc: "2.0".to_string(), id: req.id, result: Some(serde_json::to_value(result).unwrap()), error: None }
                } else {
                    let result = StreamResult {
                        success: false, stream_url: None, headers: None, subtitles: vec![],
                        audios: HashMap::new(),
                        error_code: Some("NO_STREAM".to_string()),
                        error_message: Some("No playable mirrors found".to_string()),
                        provider: Some("spectre".to_string()), proxy_port: None,
                        self_proxy: false, session_id: "".to_string(),
                    };
                    RpcResponse { jsonrpc: "2.0".to_string(), id: req.id, result: Some(serde_json::to_value(result).unwrap()), error: None }
                }
            },
            "healthCheck" => {
                RpcResponse { jsonrpc: "2.0".to_string(), id: req.id, result: Some(serde_json::json!({"ok": true, "version": "1.0.0"})), error: None }
            },
            _ => {
                RpcResponse { jsonrpc: "2.0".to_string(), id: req.id, result: None, error: Some(RpcError { code: -32601, message: "Method not found".to_string() }), }
            }
        };

        println!("{}", serde_json::to_string(&response).unwrap());
    }
    Ok(())
}
