use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT, REFERER, ORIGIN};
use serde_json::Value;

use crate::crypto;
use crate::wasm::WasmDecryptor;
use crate::models::{StreamResult, StreamHeaders, Subtitle, VideasyResponse};

use std::sync::Arc;

pub async fn resolve_videasy(
    client: &reqwest::Client,
    decryptor: Arc<WasmDecryptor>,
    tmdb_id: u64,
    media_type: &str,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<StreamResult> {
    // 1. Fetch metadata from DB Videasy
    let tmdb_url = format!("https://db.videasy.net/3/{}/{}?append_to_response=external_ids", media_type, tmdb_id);

    let tmdb_resp: Value = client.get(&tmdb_url).send().await?.json().await?;
    
    let title = tmdb_resp["title"].as_str()
        .or_else(|| tmdb_resp["name"].as_str())
        .unwrap_or("")
        .to_string();
        
    let year = tmdb_resp["release_date"].as_str()
        .or_else(|| tmdb_resp["first_air_date"].as_str())
        .unwrap_or("")
        .split('-').next().unwrap_or("").to_string();
        
    let imdb_id = tmdb_resp["external_ids"]["imdb_id"].as_str().unwrap_or("").to_string();

    // 2. Query Videasy API
    let mut params = vec![
        ("title", title),
        ("mediaType", media_type.to_string()),
        ("year", year),
        ("tmdbId", tmdb_id.to_string()),
        ("imdbId", imdb_id),
    ];
    
    if media_type == "tv" {
        params.push(("seasonId", season.unwrap_or(1).to_string()));
        params.push(("episodeId", episode.unwrap_or(1).to_string()));
        let total_seasons = tmdb_resp["number_of_seasons"].as_u64().unwrap_or(1);
        params.push(("totalSeasons", total_seasons.to_string()));
    }

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"));
    headers.insert(REFERER, HeaderValue::from_static("https://player.videasy.net/"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://player.videasy.net"));

    let providers = vec![
        "mb-flix", "meine", "overflix", "visioncine", 
        "hdmovie", "cuevana", "primewire", "1movies", 
        "primesrcme", "m4uhd", "cdn", "superflix", 
        "moviebox", "lamovie"
    ];

    let mut tasks = Vec::new();

    for provider in providers {
        let p = provider.to_string();
        let client_cl = client.clone();
        let decryptor_cl = Arc::clone(&decryptor);
        let params_cl = params.clone();
        let headers_cl = headers.clone();
        
        tasks.push(tokio::spawn(async move {
            let url = format!("https://api.videasy.net/{}/sources-with-title", p);
            
            let encrypted_hex = client_cl.get(&url)
                .query(&params_cl)
                .headers(headers_cl)
                .send().await.ok()?
                .text().await.ok()?
                .trim()
                .to_string();
                
            if encrypted_hex.is_empty() || encrypted_hex.contains("Not found") {
                return None;
            }
            
            let b64_str = decryptor_cl.decrypt(&encrypted_hex, tmdb_id as f64).ok()?;
            let json_str = crate::crypto::decrypt_cryptojs_aes(&b64_str, "").ok()?;
            
            let mut resp: VideasyResponse = serde_json::from_str(&json_str).ok()?;
            
            for s in &mut resp.sources {
                s.provider = Some(p.clone());
            }
            
            Some(resp)
        }));
    }

    let mut all_sources = Vec::new();
    let mut all_subtitles = Vec::new();

    for task in tasks {
        if let Ok(Some(resp)) = task.await {
            all_sources.extend(resp.sources);
            all_subtitles.extend(resp.subtitles);
        }
    }

    if all_sources.is_empty() {
        return Ok(StreamResult::error("NO_STREAM", "Videasy API returned no content across all providers"));
    }
    
    let resp = VideasyResponse {
        sources: all_sources,
        subtitles: all_subtitles,
    };

    // 4. Select the best source (prioritize 1080p, Auto, 4K, 720p)
    let mut best_source = None;
    let priorities = ["4K", "1080p", "Auto", "720p", "Vimeos", "480p", "360p"];
    
    for p in priorities {
        if let Some(src) = resp.sources.iter().find(|s| s.quality.as_deref() == Some(p)) {
            best_source = Some(src.url.clone());
            break;
        }
    }
    
    // Fallback to the first source if no priority matched
    let stream_url = best_source.unwrap_or_else(|| resp.sources[0].url.clone());

    // Map all sources to qualities grouped under 'Original Audio' with 'Server' prefix to match Spectre
    let mut server_list = std::collections::HashMap::new();
    
    for src in &resp.sources {
        let q = src.quality.as_deref().unwrap_or("Unknown").to_string();
        let p = src.provider.as_deref().unwrap_or("cdn").to_uppercase();
        
        let label = format!("Server {} - {}", p, q);
        server_list.insert(label, src.url.clone());
    }
    
    // Add 'Auto' or 'best' to the provider that holds the primary stream_url
    if let Some(best_src) = resp.sources.iter().find(|s| s.url == stream_url) {
        let p = best_src.provider.as_deref().unwrap_or("cdn").to_uppercase();
        let label = format!("Server {} - Auto", p);
        server_list.insert(label, stream_url.clone());
    }

    let mut audios = std::collections::HashMap::new();
    audios.insert("Original Audio".to_string(), server_list);

    // 5. Map subtitles
    let subtitles: Vec<Subtitle> = resp.subtitles.into_iter().map(|s| Subtitle {
        url: s.url,
        language: s.language,
    }).collect();

    Ok(StreamResult {
        success: true,
        stream_url: Some(stream_url),
        headers: Some(StreamHeaders {
            referer: Some("https://player.videasy.net/".to_string()),
            origin: Some("https://player.videasy.net".to_string()),
            user_agent: None,
        }),
        subtitles: Some(subtitles),
        audios: Some(audios),
        error_code: None,
        error_message: None,
        proxy_port: None,
        session_id: None,
        self_proxy: None,
    })
}
