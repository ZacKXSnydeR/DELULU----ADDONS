use anyhow::Result;
use wreq::header::{HeaderMap, HeaderValue, REFERER, USER_AGENT};
use regex::Regex;

pub struct Resolver {
    client: wreq::Client,
}

impl Resolver {
    pub fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"));
        
        Self {
            client: wreq::Client::builder()
                .emulation(wreq_util::Emulation::Chrome120)
                .cookie_store(true)
                .default_headers(headers)
                .build()
                .unwrap(),
        }
    }

    pub async fn resolve_vidsrc_to(&self, _tmdb_id: &str, _media_type: &str, _season: u32, _episode: u32) -> Result<Vec<String>> {
        // Disabled since we only have one mirror
        Ok(vec![])
    }

    pub async fn resolve_vidsrc_me(&self, tmdb_id: &str, media_type: &str, season: u32, episode: u32) -> Result<Vec<String>> {
        let mirrors = vec![
            "vsembed.su",
            "vsembed.ru",
            "vidsrcme.ru",
            "vidsrcme.su",
            "vidsrc-me.ru",
            "vidsrc-me.su",
            "vsrc.su",
            "vidsrc-embed.ru",
            "vidsrc-embed.su"
        ];

        for mirror in mirrors {
            let url = if media_type == "movie" {
                format!("https://{}/embed/movie/{}", mirror, tmdb_id)
            } else {
                format!("https://{}/embed/tv/{}/{}/{}", mirror, tmdb_id, season, episode)
            };

            let html = match self.client.get(&url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            
            let rcp_re = Regex::new(r#"src=["'](//cloudorchestranova\.com/rcp/[^"']+)["']"#).unwrap();
            let rcp_url = match rcp_re.captures(&html) {
                Some(cap) => format!("https:{}", cap.get(1).unwrap().as_str()),
                None => continue,
            };

            let html2 = match self.client.get(&rcp_url).header(REFERER, &url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            
            let prorcp_re = Regex::new(r#"(/prorcp/[^"',\s]+)"#).unwrap();
            let url3 = if let Some(cap) = prorcp_re.captures(&html2) {
                format!("https://cloudorchestranova.com{}", cap.get(1).unwrap().as_str())
            } else {
                let loc_re = Regex::new(r#"window\.location\.href\s*=\s*['"]([^'"]+)['"]"#).unwrap();
                match loc_re.captures(&html2) {
                    Some(cap) => {
                        let u = cap.get(1).unwrap().as_str();
                        if u.starts_with("/") { format!("https://cloudorchestranova.com{}", u) } else { u.to_string() }
                    },
                    None => continue,
                }
            };

            let html3 = match self.client.get(&url3).header(REFERER, &rcp_url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(t) => t,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            
            if let Ok(streams) = self.extract_streams(&html3) {
                if !streams.is_empty() {
                    return Ok(streams);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(vec![])
    }

    fn extract_streams(&self, html: &str) -> Result<Vec<String>> {
        let mut domains = vec![];
        let dom_re = Regex::new(r"test_doms\s*=\s*\[(.*?)\]")?;
        if let Some(cap) = dom_re.captures(html) {
            let inner = cap.get(1).unwrap().as_str();
            let dom_strs_re = Regex::new(r#"["'](https?://[^"']+)["']"#)?;
            for d_cap in dom_strs_re.captures_iter(inner) {
                let d = d_cap.get(1).unwrap().as_str();
                let b_re = Regex::new(r"https?://[^\.]+\.(.+)")?;
                if let Some(b_cap) = b_re.captures(d) {
                    domains.push(b_cap.get(1).unwrap().as_str().to_string());
                }
            }
        }

        if domains.is_empty() {
            domains = vec!["neonhorizonworkshops.com".into(), "wanderlynest.com".into(), "orchidpixelgardens.com".into()];
        }

        // Search for m3u8 patterns including those inside script tags or arrays
        let m3u8_re = Regex::new(r"(https?://[^\s'<>\]]+?\.m3u8[^\s'<>\]]*?)")?;
        let mut streams = vec![];
        for m_cap in m3u8_re.captures_iter(html) {
            let mut final_url = m_cap.get(1).unwrap().as_str().to_string();
            
            // Apply domain rotation if template variables like {v1} are present
            for (idx, dom) in domains.iter().enumerate() {
                let target = format!("{{v{}}}", idx + 1);
                if final_url.contains(&target) {
                    final_url = final_url.replace(&target, dom);
                }
            }
            
            // Final cleanup for any remaining {vX} placeholders
            let v_any_re = Regex::new(r"\{v\d+\}")?;
            if v_any_re.is_match(&final_url) {
                final_url = v_any_re.replace_all(&final_url, domains[0].as_str()).to_string();
            }
            
            if !streams.contains(&final_url) {
                streams.push(final_url);
            }
        }

        Ok(streams)
    }
}
