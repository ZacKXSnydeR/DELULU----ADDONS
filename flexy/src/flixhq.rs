use reqwest::Client;
use scraper::{Html, Selector};
use strsim::jaro_winkler;
use serde_json::Value;

use crate::models::TmdbInfo;
use crate::http;

pub struct FlixHqResult {
    pub title: String,
    pub link: String,
    pub year: i32,
    pub runtime: u32,
    pub media_type: String,
}

pub async fn search_flixhq(
    client: &Client,
    query: &str,
) -> Result<Vec<FlixHqResult>, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("https://flixhq.one/search?keyword={}", urlencoding::encode(query));
    let html = http::http_get(client, &url).await?;
    
    let document = Html::parse_document(&html);
    let item_sel = Selector::parse(".flw-item").unwrap();
    let title_sel = Selector::parse(".film-name a").unwrap();
    let fdi_item_sel = Selector::parse(".fdi-item").unwrap();
    let type_sel = Selector::parse(".fdi-type").unwrap();

    let mut results = Vec::new();

    for item in document.select(&item_sel) {
        if let Some(title_el) = item.select(&title_sel).next() {
            let title = title_el.text().collect::<Vec<_>>().join("").trim().to_string();
            let mut link = title_el.value().attr("href").unwrap_or("").to_string();
            if !link.starts_with("http") {
                link = format!("https://flixhq.one{}", link);
            }

            let mut year = 0;
            let mut runtime = 0;

            for info in item.select(&fdi_item_sel) {
                let text = info.text().collect::<Vec<_>>().join("").trim().to_string();
                if text.len() == 4 && text.chars().all(char::is_numeric) {
                    year = text.parse().unwrap_or(0);
                } else if text.ends_with('m') {
                    let num_str = &text[..text.len() - 1];
                    if let Ok(num) = num_str.parse::<u32>() {
                        runtime = num;
                    }
                }
            }

            let mut media_type = "movie".to_string();
            if let Some(type_el) = item.select(&type_sel).next() {
                media_type = type_el.text().collect::<Vec<_>>().join("").trim().to_lowercase();
            }

            results.push(FlixHqResult {
                title,
                link,
                year,
                runtime,
                media_type,
            });
        }
    }

    Ok(results)
}

pub fn score_results(tmdb: &TmdbInfo, results: Vec<FlixHqResult>) -> Vec<FlixHqResult> {
    let tmdb_dur = tmdb.runtime.unwrap_or(0);
    
    let mut scored: Vec<(f64, FlixHqResult)> = Vec::new();

    for (idx, res) in results.into_iter().enumerate() {
        if tmdb.media_type == "movie" && tmdb_dur > 0 && res.runtime > 0 {
            if (tmdb_dur as i32 - res.runtime as i32).abs() > 60 {
                continue;
            }
        }

        let mut score = 0.0;
        let similarity = jaro_winkler(&tmdb.title.to_lowercase(), &res.title.to_lowercase());
        score += similarity * 2000.0;

        if tmdb.media_type == res.media_type {
            score += 500.0;
        }

        score += 100.0 - (idx as f64).min(50.0);

        if let Some(y) = tmdb.year {
            if res.year > 0 {
                if y == res.year {
                    score += 800.0;
                } else if (y - res.year).abs() <= 1 {
                    score += 300.0;
                }
            }
        }

        if tmdb.media_type == "movie" && tmdb_dur > 0 && res.runtime > 0 {
            if (tmdb_dur as i32 - res.runtime as i32).abs() <= 10 {
                score += 1000.0;
            }
        }

        scored.push((score, res));
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.into_iter().map(|(_, r)| r).collect()
}

pub async fn extract_episode_link(
    client: &Client,
    series_url: &str,
    season: u32,
    episode: u32,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let html = http::http_get(client, series_url).await?;
    let document = Html::parse_document(&html);
    let eps_sel = Selector::parse(".eps-item").unwrap();

    let target_str = format!("s{:02}-e{:02}", season, episode);

    for ep_el in document.select(&eps_sel) {
        if let Some(link) = ep_el.value().attr("href") {
            if link.contains(&target_str) {
                let mut final_link = link.to_string();
                if !final_link.starts_with("http") {
                    final_link = format!("https://flixhq.one{}", final_link);
                }
                return Ok(Some(final_link));
            }
        }
    }
    Ok(None)
}

pub async fn get_servers(
    client: &Client,
    watch_url: &str,
    is_tv: bool,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let html = http::http_get(client, watch_url).await?;
    
    let mut token = None;
    let document = Html::parse_document(&html);

    if is_tv {
        if let Ok(sel) = Selector::parse("#series-player") {
            if let Some(el) = document.select(&sel).next() {
                token = el.value().attr("data-token").map(String::from);
            }
        }
    } else {
        if let Ok(sel) = Selector::parse("#main-wrapper") {
            if let Some(el) = document.select(&sel).next() {
                token = el.value().attr("data-token").map(String::from);
            }
        }
    }

    if token.is_none() {
        let re = regex::Regex::new(r#"data-token=["']([^"']+)["']"#).unwrap();
        if let Some(cap) = re.captures(&html) {
            token = Some(cap[1].to_string());
        }
    }

    let token = token.ok_or("No data-token found on page")?;

    let ajax_url = "https://flixhq.one/ajax/ajax.php";
    let mut params = std::collections::HashMap::new();
    if is_tv {
        params.insert("players_show", token);
    } else {
        params.insert("players", token);
    }

    let res = client.post(ajax_url)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Referer", watch_url)
        .form(&params)
        .send()
        .await?;

    let json: Value = res.json().await?;
    
    if let Some(arr) = json.as_array() {
        Ok(arr.clone())
    } else {
        Ok(vec![json])
    }
}
