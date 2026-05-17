use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

type HmacMd5 = Hmac<Md5>;

pub const SECRET_KEY_DEFAULT: &str = "76iRl07s0xSN9jqmEWAt79EBJZulIQIsV64FZr2O";
pub const SIGNATURE_BODY_MAX_BYTES: usize = 102_400;

pub fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub fn generate_x_client_token(timestamp_ms: u64) -> String {
    let ts_str = timestamp_ms.to_string();
    let reversed_ts: String = ts_str.chars().rev().collect();
    let hash_val = md5_hex(reversed_ts.as_bytes());
    format!("{},{}", ts_str, hash_val)
}

fn get_sorted_query_string(url_str: &str) -> String {
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => return String::new(),
    };

    let mut params = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        params.entry(k.into_owned()).or_insert_with(Vec::new).push(v.into_owned());
    }

    let mut parts = Vec::new();
    for (key, values) in params {
        for value in values {
            parts.push(format!("{}={}", key, value));
        }
    }
    parts.join("&")
}

pub fn build_canonical_string(
    method: &str,
    accept: Option<&str>,
    content_type: Option<&str>,
    url_str: &str,
    body: Option<&str>,
    timestamp_ms: u64,
) -> String {
    let url = Url::parse(url_str).unwrap();
    let path = url.path();
    let query = get_sorted_query_string(url_str);
    let canonical_url = if query.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, query)
    };

    let (body_hash, body_length) = if let Some(b) = body {
        let b_bytes = b.as_bytes();
        let truncated = if b_bytes.len() > SIGNATURE_BODY_MAX_BYTES {
            &b_bytes[..SIGNATURE_BODY_MAX_BYTES]
        } else {
            b_bytes
        };
        (md5_hex(truncated), b_bytes.len().to_string())
    } else {
        (String::new(), String::new())
    };

    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(),
        accept.unwrap_or(""),
        content_type.unwrap_or(""),
        body_length,
        timestamp_ms,
        body_hash,
        canonical_url
    )
}

pub fn generate_x_tr_signature(
    method: &str,
    accept: Option<&str>,
    content_type: Option<&str>,
    url_str: &str,
    body: Option<&str>,
    timestamp_ms: u64,
) -> String {
    let canonical = build_canonical_string(method, accept, content_type, url_str, body, timestamp_ms);
    let secret_bytes = STANDARD.decode(SECRET_KEY_DEFAULT).expect("Failed to decode secret key");

    let mut mac = HmacMd5::new_from_slice(&secret_bytes).expect("HMAC can take key of any size");
    mac.update(canonical.as_bytes());
    let sig_b64 = STANDARD.encode(mac.finalize().into_bytes());

    format!("{}|2|{}", timestamp_ms, sig_b64)
}

pub fn get_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
