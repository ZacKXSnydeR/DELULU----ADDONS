//! TMDB metadata fetcher with auto-detection of media type and episode offsets.

use crate::models::TmdbInfo;
use reqwest::Client;
use serde_json::Value;

/// Fetch TMDB info with auto-type detection and absolute episode calculation.
pub async fn fetch_tmdb_info(
    client: &Client,
    tmdb_id: u64,
    media_type: &str,
    requested_season: Option<u32>,
    requested_episode: Option<u32>,
    api_key: &str,
) -> Result<TmdbInfo, Box<dyn std::error::Error + Send + Sync>> {
    let types_to_try = match media_type {
        "movie" => vec!["movie"],
        "tv" => vec!["tv"],
        _ => vec!["movie", "tv"], // auto-detect
    };

    let mut last_err = None;

    for mt in &types_to_try {
        let url = format!(
            "https://api.themoviedb.org/3/{}/{}?api_key={}&language=en-US",
            mt, tmdb_id, api_key
        );

        match client.get(&url).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    last_err = Some(format!("TMDB {} returned {}", mt, resp.status()));
                    continue;
                }

                let json: Value = resp.json().await?;
                let title = json["title"]
                    .as_str()
                    .or(json["name"].as_str())
                    .unwrap_or("")
                    .to_string();

                let original_title = json["original_title"]
                    .as_str()
                    .or(json["original_name"].as_str())
                    .unwrap_or(&title)
                    .to_string();

                let release_date = json["release_date"]
                    .as_str()
                    .or(json["first_air_date"].as_str())
                    .unwrap_or("");

                let year = if release_date.len() >= 4 {
                    release_date[0..4].parse::<i32>().ok()
                } else {
                    None
                };

                let runtime = if *mt == "movie" {
                    json["runtime"].as_u64().map(|r| r as u32)
                } else {
                    json["episode_run_time"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_u64())
                        .map(|r| r as u32)
                };

                // Calculate absolute episode offset for TV shows (for Anime support)
                let mut absolute_episode_offset = 0;
                if *mt == "tv" {
                    if let Some(target_se) = requested_season {
                        if target_se > 1 {
                            if let Some(seasons) = json["seasons"].as_array() {
                                for s in seasons {
                                    let sn = s["season_number"].as_u64().unwrap_or(0);
                                    if sn > 0 && sn < target_se as u64 {
                                        absolute_episode_offset += s["episode_count"].as_u64().unwrap_or(0) as u32;
                                    }
                                }
                            }
                        }
                    }
                }

                eprintln!(
                    "[tmdb] Resolved: \"{}\" ({}) year={:?} runtime={:?} offset={}",
                    title, mt, year, runtime, absolute_episode_offset
                );

                return Ok(TmdbInfo {
                    _tmdb_id: tmdb_id,
                    title,
                    original_title,
                    year,
                    runtime,
                    media_type: mt.to_string(),
                    season: requested_season,
                    episode: requested_episode,
                    absolute_episode_offset,
                });
            }
            Err(e) => {
                last_err = Some(format!("TMDB request failed: {}", e));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| "TMDB fetch failed".into()).into())
}
