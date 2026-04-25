use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[serde(rename = "jsonrpc")]
    pub _jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveParams {
    pub media_type: String,
    pub tmdb_id: serde_json::Value,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamResult {
    pub success: bool,
    pub stream_url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub subtitles: Vec<Subtitle>,
    /// Format: { "Original Audio": { "Server 1": "url", "Server 2": "url" } }
    pub audios: HashMap<String, HashMap<String, String>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub provider: Option<String>,
    pub proxy_port: Option<u16>,
    pub self_proxy: bool,
    pub session_id: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct Subtitle {
    pub url: String,
    pub language: String,
}
