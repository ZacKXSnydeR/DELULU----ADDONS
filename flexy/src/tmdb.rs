use crate::models::TmdbInfo;
use reqwest::Client;
use serde_json::Value;

pub async fn fetch_tmdb_info(
    client: &Client,
    tmdb_id: u64,
    media_type: &str,
    requested_season: Option<u32>,
    requested_episode: Option<u32>,
    api_key: &str,
) -> Result<TmdbInfo, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "https://api.themoviedb.org/3/{}/{}?api_key={}&language=en-US",
        media_type, tmdb_id, api_key
    );

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(format!("TMDB returned {}", resp.status()).into());
    }

    let json: Value = resp.json().await?;
    let title = json["title"]
        .as_str()
        .or(json["name"].as_str())
        .unwrap_or("")
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

    let runtime = if media_type == "movie" {
        json["runtime"].as_u64().map(|r| r as u32)
    } else {
        json["episode_run_time"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_u64())
            .map(|r| r as u32)
    };

    eprintln!(
        "[tmdb] Resolved: \"{}\" ({}) year={:?} runtime={:?}",
        title, media_type, year, runtime
    );

    Ok(TmdbInfo {
        _tmdb_id: tmdb_id,
        title,
        year,
        runtime,
        media_type: media_type.to_string(),
        season: requested_season,
        episode: requested_episode,
    })
}
