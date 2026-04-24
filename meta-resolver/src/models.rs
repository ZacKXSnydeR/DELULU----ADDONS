use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub result: Option<T>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

/// Standard Delulu Stream Request (for Trailer Playback)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveStreamRequest {
    pub media_type: String,
    pub tmdb_id: u32,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

/// Standard Delulu Stream Result
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolveStreamResult {
    pub success: bool,
    pub stream_url: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// New: ID Mapping Request (for Middleware usage)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveIdRequest {
    pub media_type: String,
    pub tmdb_id: u32,
}

/// New: ID Mapping Result
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolveIdResult {
    pub success: bool,
    pub tmdb_id: u32,
    pub imdb_id: Option<String>,
    pub error: Option<String>,
}

/// Batch Request (Parallel Processing)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResolveRequest {
    pub items: Vec<ResolveStreamRequest>, // Can be used for IDs or Streams
    pub workers: Option<usize>,
}
