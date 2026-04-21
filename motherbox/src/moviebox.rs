//! MovieBox provider — pure HTTP resolver.
//! Uses shared http.rs client, Jaro-Winkler search scoring,
//! improved TV season/episode mapping (Anime support), and 401 auto-retry.

use crate::auth::{self, Auth, MOVIEBOX_HOST, PLAYER_HOST, UA_MOBILE};
use crate::http;
use crate::models::{MovieBoxInfo, Subtitle, TmdbInfo};
use crate::state;
use regex::Regex;
use reqwest::Client;
use serde_json::{json, Value};
use strsim::jaro_winkler;

/// H5 API base URL — used for detail and caption endpoints (play moved to PLAYER_HOST).
const H5_API: &str = "https://h5-api.aoneroom.com";

/// Daily-limit error codes from MovieBox API.
const DAILY_LIMIT_CODES: &[i64] = &[10007, 10016, 10017, 40301, 40302];

/// Slugify text for MovieBox search keywords.
pub fn slugify(text: &str) -> String {
    let re = Regex::new(r"[^a-z0-9]+").unwrap();
    re.replace_all(&text.to_lowercase().trim(), "-")
        .trim_matches('-')
        .to_string()
}

/// Search MovieBox for a specific title using TMDB metadata for scoring.
/// Returns the `detail/...` path for the best match.
pub async fn search_detail_path(
    client: &Client,
    tmdb: &TmdbInfo,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Generate multiple search variations to maximize hit rate
    let mut variations = vec![tmdb.title.clone()];
    if let Some(year) = tmdb.year {
        variations.push(format!("{} {}", tmdb.title, year));
    }
    if tmdb.original_title != tmdb.title {
        variations.push(tmdb.original_title.clone());
    }

    let title_l = tmdb.title.to_lowercase();
    let tmdb_dur = tmdb.runtime.unwrap_or(0) * 60;
    let expected_st = if tmdb.media_type == "tv" { 2 } else { 1 };

    for variant in &variations {
        for page in 1..=2 { // Probe 2 pages for maximum depth
            let slug = slugify(variant);
            let url = if page == 1 {
                format!("{}/web/searchResult?keyword={}", MOVIEBOX_HOST, urlencoding::encode(&slug))
            } else {
                format!("{}/web/searchResult?keyword={}&page={}", MOVIEBOX_HOST, urlencoding::encode(&slug), page)
            };

            eprintln!("[moviebox] Searching (P{}): \"{}\" → {}", page, variant, &url[..url.len().min(80)]);

            let html = match http::http_get(client, &url).await {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("[moviebox] Search request failed: {}", e);
                    continue;
                }
            };

            let re = Regex::new(r#"id="__NUXT_DATA__"[^>]*>(.*?)</script>"#).unwrap();
            let cap = match re.captures(&html) {
                Some(c) => c,
                None => continue,
            };

            let raw: Vec<Value> = match serde_json::from_str(&cap[1]) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[moviebox] Nuxt JSON parse error: {}", e);
                    continue;
                }
            };

            // Nuxt hydration: integer refs point to other slots in the array
            let hydrate = |val: &Value| -> Value {
                if let Some(i) = val.as_u64() {
                    if (i as usize) < raw.len() {
                        return raw[i as usize].clone();
                    }
                }
                val.clone()
            };

            let mut results = Vec::new();
            for item in &raw {
                if let Some(obj) = item.as_object() {
                    if obj.contains_key("subjectId") && obj.contains_key("title") {
                        let mut hydrated = serde_json::Map::new();
                        for (k, v) in obj {
                            hydrated.insert(k.clone(), hydrate(v));
                        }
                        results.push(Value::Object(hydrated));
                    }
                }
            }

            if results.is_empty() {
                continue;
            }

            let mut scored_results: Vec<(f64, &Value)> = results
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let t = item["title"].as_str().unwrap_or("").to_lowercase();
                    let st = item["subjectType"].as_u64().unwrap_or(1);
                    let mb_dur = item["duration"].as_u64().unwrap_or(0);
                    let rd = item["releaseDate"].as_str().unwrap_or("");
                    let mb_year = if rd.len() >= 4 { &rd[0..4] } else { "" };

                    let mut score: f64 = 0.0;

                    // 1. Jaro-Winkler Title Matching (0.0 to 1.0 -> max 2000)
                    let similarity = jaro_winkler(&title_l, &t);
                    score += similarity * 2000.0;

                    // 2. Content type match preference
                    if st == expected_st as u64 {
                        score += 500.0;
                    } else if st == 6 {
                        score -= 2000.0; // Music penalty
                    }

                    // 3. API order bonus
                    score += 100.0 - (index as f64).min(50.0);

                    // 4. Release year verification
                    if let Some(y) = tmdb.year {
                        let y_str = y.to_string();
                        if mb_year == y_str {
                            score += 800.0;
                        } else if !mb_year.is_empty() {
                            if let Ok(m_year) = mb_year.parse::<i32>() {
                                if (m_year - y).abs() <= 1 {
                                    score += 300.0;
                                }
                            }
                        }
                    }

                    // 5. Duration fingerprinting (±2min exact, ±10min partial)
                    if tmdb.media_type == "movie" && tmdb_dur > 0 && mb_dur > 0 {
                        let diff = (tmdb_dur as i32 - mb_dur as i32).abs();
                        if diff <= 120 {
                            score += 1000.0;
                        } else if diff <= 600 {
                            score += 300.0;
                        }
                    }

                    (score, item)
                })
                .collect();

            scored_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

            // Log top match
            if let Some((score, best)) = scored_results.first() {
                eprintln!(
                    "[moviebox]   Top match: \"{}\" score={:.2} id={}",
                    best["title"].as_str().unwrap_or("?"),
                    score,
                    best["subjectId"].as_str().unwrap_or("?")
                );

                if *score > 2500.0 {
                    if let Some(path) = best["detailPath"].as_str() {
                        eprintln!("[moviebox] Selected: \"{}\" (score={:.2})", best["title"].as_str().unwrap_or("?"), score);
                        return Ok(format!("detail/{}", path));
                    }
                }
            }
        }
    }

    Err("No matching content found on MovieBox".into())
}

/// Map TMDB season/episode to MovieBox's internal 1-indexed `se`/`ep`.
///
/// Improved logic handles absolute episode numbering for Anime/long-running shows.
pub fn map_episode_indices(
    tmdb: &TmdbInfo,
    mb_seasons: &[Value],
) -> (u32, u32) {
    let target_se = tmdb.season.unwrap_or(1);
    let target_ep = tmdb.episode.unwrap_or(1);

    if mb_seasons.is_empty() {
        // Fallback to 1-indexed parameters
        return (target_se, target_ep);
    }

    // 1. Try exact season match using the 'se' field
    for season in mb_seasons {
        let sn = season["se"]
            .as_u64()
            .or(season["seasonNo"].as_u64())
            .or(season["seasonNumber"].as_u64());

        if let Some(sn_val) = sn {
            if sn_val as u32 == target_se {
                return (sn_val as u32, target_ep);
            }
        }
    }

    // 2. Absolute Episode Mapping (Fallback for Anime)
    // If the requested season is > 1 but MovieBox only has 1 season (usually Anime "dump").
    if mb_seasons.len() == 1 {
        let sn = mb_seasons[0]["se"].as_u64().unwrap_or(1) as u32;
        let abs_ep = tmdb.absolute_episode_offset + target_ep;
        let mb_max_ep = mb_seasons[0]["maxEp"].as_u64().unwrap_or(0) as u32;
        
        if mb_max_ep >= abs_ep {
            eprintln!(
                "[moviebox] Absolute mapping used: S{}E{} (abs={}) -> MB se={}, ep={}",
                target_se, target_ep, abs_ep, sn, abs_ep
            );
            return (sn, abs_ep);
        }
    }

    // 3. Naive Fallback (1-indexed)
    (target_se, target_ep)
}

/// Fetch detailed metadata for a subject.
pub async fn get_detail(
    client: &Client,
    detail_path: &str,
) -> Result<MovieBoxInfo, Box<dyn std::error::Error + Send + Sync>> {
    let normalized = detail_path.trim_start_matches('/').replace("detail/", "");
    let url = format!("{}/wefeed-h5api-bff/detail?detailPath={}", H5_API, normalized);

    let body = http::http_get(client, &url).await?;
    let resp: Value = serde_json::from_str(&body)?;

    if resp["code"] != 0 {
        return Err(format!("Detail API error code: {}", resp["code"]).into());
    }

    let data = &resp["data"];
    let subj = &data["subject"];
    let resource = &data["resource"];

    Ok(MovieBoxInfo {
        detail_path: normalized,
        subject_id: subj["subjectId"].as_str().unwrap_or("").to_string(),
        media_type: if subj["subjectType"] == 2 {
            "tv".to_string()
        } else {
            "movie".to_string()
        },
        seasons: resource["seasons"].as_array().cloned().unwrap_or_default(),
        dubs: subj["dubs"].as_array().cloned().unwrap_or_default(),
    })
}

/// Fetch all available stream links for a subject.
pub async fn get_play_links(
    client: &Client,
    subject_id: &str,
    detail_path: &str,
    se: u32,
    ep: u32,
    auth: &Auth,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let normalized = detail_path.trim_start_matches('/').replace("detail/", "");
    let url = format!(
        "{}/wefeed-h5api-bff/subject/play?subjectId={}&se={}&ep={}&detailPath={}",
        PLAYER_HOST, subject_id, se, ep, normalized
    );

    let client_info = json!({"timezone": "Asia/Dhaka"});
    let referer = format!(
        "{}/spa/videoPlayPage/movies/{}?id={}&type=/movie/detail&detailSe=&detailEp=&lang=en",
        PLAYER_HOST, normalized, subject_id
    );

    eprintln!("[moviebox] Play API: se={}, ep={}, subject={}", se, ep, subject_id);

    let resp = client
        .get(&url)
        .header("Cookie", format!("uuid={}; token={}", auth.uuid, auth.token))
        .header("User-Agent", UA_MOBILE)
        .header("Referer", &referer)
        .header("x-client-info", client_info.to_string())
        .header("Accept", "application/json")
        .header("sec-ch-ua-mobile", "?1")
        .header("sec-ch-ua-platform", "\"Android\"")
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?
        .text()
        .await?;

    let json: Value = serde_json::from_str(&resp)?;
    let code = json["code"].as_i64().unwrap_or(-1);

    if code == 0 {
        let data = &json["data"];
        let mut all_links = Vec::new();
        for key in ["streams", "hls", "dash"] {
            if let Some(links) = data[key].as_array() {
                all_links.extend(links.iter().cloned());
            }
        }
        eprintln!("[moviebox] Play API returned {} links", all_links.len());
        return Ok(all_links);
    }

    // Check for daily limit
    if DAILY_LIMIT_CODES.contains(&code) {
        let mut st = state::load_state();
        st.mark_daily_limit();
        state::save_state(&st);
        return Err(format!("MovieBox daily limit hit (code {})", code).into());
    }

    // Check for auth failure
    if code == 401 {
        return Err("AUTH_EXPIRED".into());
    }

    Err(format!("Play API error code: {} — {}", code, json["msg"].as_str().unwrap_or("unknown")).into())
}

/// Fetch all available stream links with 401 auto-retry.
pub async fn get_play_links_with_retry(
    client: &Client,
    subject_id: &str,
    detail_path: &str,
    se: u32,
    ep: u32,
    auth: &Auth,
) -> Result<(Vec<Value>, Auth), Box<dyn std::error::Error + Send + Sync>> {
    match get_play_links(client, subject_id, detail_path, se, ep, auth).await {
        Ok(links) if !links.is_empty() => Ok((links, auth.clone())),
        Ok(_) => {
            eprintln!("[moviebox] Empty links with cached token — forcing credential refresh...");
            let fresh_auth = auth::force_refresh(client)
                .await
                .ok_or("Failed to refresh credentials after empty links")?;
            let links = get_play_links(client, subject_id, detail_path, se, ep, &fresh_auth).await?;
            if links.is_empty() {
                return Err("Play API returned empty links after credential refresh".into());
            }
            Ok((links, fresh_auth))
        }
        Err(e) if e.to_string() == "AUTH_EXPIRED" => {
            eprintln!("[moviebox] 401 — attempting credential refresh and retry...");
            let fresh_auth = auth::force_refresh(client)
                .await
                .ok_or("Failed to refresh credentials after 401")?;
            let links = get_play_links(client, subject_id, detail_path, se, ep, &fresh_auth).await?;
            Ok((links, fresh_auth))
        }
        Err(e) => Err(e),
    }
}

/// Fetch available subtitles (captions) for a subject.
pub async fn get_captions(
    client: &Client,
    subject_id: &str,
    detail_path: &str,
    media_id: Option<&str>,
) -> Result<Vec<Subtitle>, Box<dyn std::error::Error + Send + Sync>> {
    let normalized = detail_path.trim_start_matches('/').replace("detail/", "");
    let mut url = format!(
        "{}/wefeed-h5api-bff/subject/caption?format=MP4&subjectId={}&detailPath={}",
        H5_API, subject_id, normalized
    );
    if let Some(id) = media_id {
        url.push_str(&format!("&id={}", id));
    }

    let client_info = json!({"timezone": "Asia/Dhaka"});
    let referer = format!(
        "{}/spa/videoPlayPage/movies/{}?id={}&type=/movie/detail&detailSe=&detailEp=&lang=en",
        PLAYER_HOST, normalized, subject_id
    );

    let body = client
        .get(&url)
        .header("User-Agent", UA_MOBILE)
        .header("Referer", referer)
        .header("x-client-info", client_info.to_string())
        .send()
        .await?
        .text()
        .await?;

    let resp: Value = serde_json::from_str(&body)?;
    let mut subtitles = Vec::new();

    if let Some(captions) = resp["data"]["captions"].as_array() {
        for cap in captions {
            if let (Some(url), Some(lang)) = (cap["url"].as_str(), cap["lanName"].as_str()) {
                subtitles.push(Subtitle {
                    url: url.to_string(),
                    language: lang.to_string(),
                });
            }
        }
    }

    eprintln!("[moviebox] Found {} subtitles", subtitles.len());
    Ok(subtitles)
}
