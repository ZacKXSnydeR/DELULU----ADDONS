use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// --- Delulu JSON-RPC 2.0 Types ---

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse<T> {
    pub id: Option<serde_json::Value>,
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveStreamParams {
    pub media_type: String,
    pub tmdb_id: u64,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamResult {
    pub success: bool,
    pub stream_url: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub subtitles: Option<Vec<Subtitle>>,
    /// Multi-audio version streams: audio_name -> { quality -> url }
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audios: Option<HashMap<String, AudioResult>>,
    /// Embedded proxy port (127.0.0.1:PORT) — set when self_proxy is true
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    /// Session ID for the embedded proxy — used for audio/quality switching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// When true, all URLs in this result are already proxied through the embedded server.
    /// The app should NOT set CDN headers — the addon handles it internally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_proxy: Option<bool>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl StreamResult {
    pub fn error(code: &str, message: &str) -> Self {
        StreamResult {
            success: false,
            error_code: Some(code.into()),
            error_message: Some(message.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct AudioResult {
    pub streams: HashMap<String, String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Subtitle {
    pub url: String,
    pub language: String,
}

/// --- MovieBox Internal API Models ---

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieBoxInfo {
    pub detail_path: String,
    pub subject_id: String,
    pub media_type: String,
    pub seasons: Vec<serde_json::Value>,
    pub dubs: Vec<serde_json::Value>,
}

/// TMDB metadata used for MovieBox search scoring.
#[derive(Debug, Clone)]
pub struct TmdbInfo {
    pub _tmdb_id: u64,
    pub title: String,
    pub original_title: String,
    pub year: Option<i32>,
    pub runtime: Option<u32>,
    pub media_type: String,
    /// For TV shows: the requested season and episode
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// The absolute episode number offset from Season 1 (for Anime)
    pub absolute_episode_offset: u32,
}
