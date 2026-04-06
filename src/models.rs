use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaType {
    Movie,
    TvShow,
    Anime,
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaType::Movie => write!(f, "Movie"),
            MediaType::TvShow => write!(f, "TV Show"),
            MediaType::Anime => write!(f, "Anime"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MediaQuery {
    pub tmdb_id: String,
    pub media_type: MediaType,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub bypass_path: Option<String>,
}

impl std::fmt::Display for MediaQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.media_type {
            MediaType::Movie => write!(f, "{}", self.tmdb_id),
            MediaType::TvShow | MediaType::Anime => write!(
                f,
                "{} (Season {}, Episode {})",
                self.tmdb_id,
                self.season.unwrap_or(0),
                self.episode.unwrap_or(0)
            ),
        }
    }
}

// ── API response models ──────────────────────────────────────────────────────

/// Top-level API response envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse {
    #[serde(alias = "data", alias = "source")]
    pub stream: Option<StreamData>,
    #[serde(default)]
    pub subtitles: Vec<Caption>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamData {
    pub playlist: Option<String>,
    #[serde(default)]
    pub captions: Vec<Caption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHeaders {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<StreamHeaders>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Caption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Flattened result handed to the UI after parsing.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<Stream>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<Caption>,
}

fn clean_stream_url(url: &str) -> String {
    if let Some((base, query)) = url.split_once('?') {
        let mut new_query = Vec::new();
        for pair in query.split('&') {
            if !pair.starts_with("headers=")
                && !pair.starts_with("referer=")
                && !pair.starts_with("origin=")
                && !pair.starts_with("s=")
            {
                new_query.push(pair);
            }
        }
        if new_query.is_empty() {
            base.to_string()
        } else {
            format!("{}?{}", base, new_query.join("&"))
        }
    } else {
        url.to_string()
    }
}

impl Output {
    pub fn from_response(resp: ApiResponse) -> Self {
        let mut subtitles = resp.subtitles;

        let streams = if let Some(st) = resp.stream {
            if !st.captions.is_empty() {
                subtitles.extend(st.captions); // Extract captions from the stream object
            }

            if let Some(playlist) = st.playlist {
                // Delulu contract: always use vidlink.pro header profile for playback.
                let headers = Some(StreamHeaders {
                    referer: Some("https://vidlink.pro/".to_string()),
                    origin: Some("https://vidlink.pro".to_string()),
                });

                vec![Stream {
                    url: Some(clean_stream_url(&playlist)),
                    quality: Some("Auto".to_string()),
                    headers,
                }]
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Normalize subtitle languages (e.g., "English - English" -> "English")
        for sub in &mut subtitles {
            if let Some(lang) = &sub.language {
                if let Some((normalized, _)) = lang.split_once(" - ") {
                    sub.language = Some(normalized.trim().to_string());
                }
            }
        }

        Self { streams, subtitles }
    }
}
