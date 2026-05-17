use crate::crypto;
use crate::models::TmdbInfo;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;

const API_BASE: &str = "https://api6.aoneroom.com";
const SEARCH_PATH: &str = "/wefeed-mobile-bff/subject-api/search";

const CLIENT_INFO: &str = r#"{"package_name":"com.community.oneroom","version_name":"3.0.03.0529.03","version_code":50020046,"os":"android","os_version":"13","install_ch":"ps","device_id":"8c730aaad202603138c730aaad2026031","install_store":"ps","gaid":"8c730aaa-d202-6031-38c7-30aaad202603","brand":"Redmi","model":"23078RKD5C","system_language":"en","net":"NETWORK_WIFI","region":"US","timezone":"Asia/Dhaka","sp_code":"40401","X-Play-Mode":"2"}"#;
const UA: &str = "com.community.oneroom/50020046 (Linux; U; Android 13; en_US; 23078RKD5C; Build/TQ2A.230405.003; Cronet/135.0.7012.3)";

const RESOURCE_PATH: &str = "/wefeed-mobile-bff/subject-api/resource";

pub async fn search_mobile(
    client: &Client,
    tmdb: &TmdbInfo,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let keyword = &tmdb.title;
    let url = format!("{}{}?keyword={}&page=1&per_page=20", API_BASE, SEARCH_PATH, urlencoding::encode(keyword));
    let ts = crypto::get_timestamp_ms();

    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("X-Client-Token".to_string(), crypto::generate_x_client_token(ts));
    headers.insert("x-tr-signature".to_string(), crypto::generate_x_tr_signature("GET", Some("application/json"), Some("application/json"), &url, None, ts));
    headers.insert("X-Client-Info".to_string(), CLIENT_INFO.to_string());
    headers.insert("X-Client-Status".to_string(), "0".to_string());

    let mut req = client.get(&url);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req.send().await?.text().await?;
    eprintln!("[v3_mobile] Raw Response ({}): {}", url, &resp[..resp.len().min(500)]);
    let json: Value = serde_json::from_str(&resp)?;

    if json["code"] != 0 {
        return Err(format!("Mobile Search API error code: {}", json["code"]).into());
    }

    let items = json["data"]["items"].as_array().cloned().unwrap_or_default();
    eprintln!("[v3_mobile] Mobile API found {} items", items.len());

    Ok(items)
}

pub async fn get_resource_mobile(
    client: &Client,
    subject_id: &str,
    se: u32,
    ep: u32,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}{}?subjectId={}&se={}&ep={}&page=1&per_page=20&resolution=0", API_BASE, RESOURCE_PATH, subject_id, se, ep);
    let ts = crypto::get_timestamp_ms();

    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), UA.to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("X-Client-Token".to_string(), crypto::generate_x_client_token(ts));
    headers.insert("x-tr-signature".to_string(), crypto::generate_x_tr_signature("GET", Some("application/json"), Some("application/json"), &url, None, ts));
    headers.insert("X-Client-Info".to_string(), CLIENT_INFO.to_string());
    headers.insert("X-Client-Status".to_string(), "0".to_string());

    let mut req = client.get(&url);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req.send().await?.text().await?;
    eprintln!("[v3_mobile] Raw Response ({}): {}", url, &resp[..resp.len().min(500)]);
    let json: Value = serde_json::from_str(&resp)?;

    if json["code"] != 0 {
        return Err(format!("Mobile Resource API error code: {}", json["code"]).into());
    }

    let items = json["data"]["items"].as_array().cloned().unwrap_or_default();
    eprintln!("[v3_mobile] Mobile Resource API found {} links", items.len());

    Ok(items)
}

