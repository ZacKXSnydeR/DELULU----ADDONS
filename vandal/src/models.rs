use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonRpcResponse<T> {
    pub id: Option<serde_json::Value>,
    pub jsonrpc: String,
    pub protocol_version: String,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamHeaders {
    #[serde(rename = "Referer", skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(rename = "Origin", skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(rename = "User-Agent", skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamResult {
    pub success: bool,
    pub stream_url: Option<String>,
    pub headers: Option<StreamHeaders>,
    pub subtitles: Option<Vec<Subtitle>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audios: Option<std::collections::HashMap<String, std::collections::HashMap<String, String>>>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_proxy: Option<bool>,
}

impl StreamResult {
    pub fn error(code: &str, message: &str) -> Self {
        StreamResult {
            success: false,
            error_code: Some(code.into()),
            error_message: Some(message.into()),
            proxy_port: None,
            self_proxy: None,
            audios: None,
            ..Default::default()
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Subtitle {
    pub url: String,
    pub language: String,
}

// Videasy specific internal structs
#[derive(Debug, Deserialize)]
pub struct VideasySource {
    pub url: String,
    pub quality: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VideasySubtitle {
    pub url: String,
    pub language: String,
}

#[derive(Debug, Deserialize)]
pub struct VideasyResponse {
    pub sources: Vec<VideasySource>,
    pub subtitles: Vec<VideasySubtitle>,
}
