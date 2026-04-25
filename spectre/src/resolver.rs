use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, REFERER, USER_AGENT};
use regex::Regex;

pub struct Resolver {
    client: reqwest::Client,
}

impl Resolver {
    pub fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"));
        
        Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .unwrap(),
        }
    }

    pub async fn resolve_vidsrc_to(&self, tmdb_id: &str, media_type: &str, season: u32, episode: u32) -> Result<Vec<String>> {
        let url = if media_type == "movie" {
            format!("https://vidsrc.to/embed/movie/{}", tmdb_id)
        } else {
            format!("https://vidsrc.to/embed/tv/{}/{}/{}", tmdb_id, season, episode)
        };

        let html = self.client.get(&url).send().await?.text().await?;
        
        let iframe_re = Regex::new(r#"<iframe[^>]+src=["']([^"']+)["']"#)?;
        let iframe_url = match iframe_re.captures(&html) {
            Some(cap) => {
                let u = cap.get(1).unwrap().as_str();
                if u.starts_with("//") { format!("https:{}", u) }
                else if u.starts_with("/") { format!("https://vidsrc.to{}", u) }
                else { u.to_string() }
            },
            None => return Ok(vec![]),
        };

        let html2 = self.client.get(&iframe_url).header(REFERER, &url).send().await?.text().await?;
        
        let hash_re = Regex::new(r#"data-hash=["']([^"']+)["']"#)?;
        let full_hash = match hash_re.captures(&html2) {
            Some(cap) => cap.get(1).unwrap().as_str(),
            None => return Ok(vec![]),
        };
        
        let final_hash = full_hash.split(':').next().unwrap_or(full_hash);
        let url3 = format!("https://cloudnestra.com/rcp/{}", final_hash);

        let html3 = self.client.get(&url3).header(REFERER, &iframe_url).send().await?.text().await?;
        
        let prorcp_re = Regex::new(r"(/prorcp/[a-zA-Z0-9_=-]+)")?;
        let url4 = if let Some(cap) = prorcp_re.captures(&html3) {
            format!("https://cloudnestra.com{}", cap.get(1).unwrap().as_str())
        } else {
            let loc_re = Regex::new(r#"window\.location\.href\s*=\s*['"]([^'"]+)['"]"#)?;
            match loc_re.captures(&html3) {
                Some(cap) => {
                    let u = cap.get(1).unwrap().as_str();
                    if u.starts_with("/") { format!("https://cloudnestra.com{}", u) } else { u.to_string() }
                },
                None => return Ok(vec![]),
            }
        };

        let html4 = self.client.get(&url4).header(REFERER, &url3).send().await?.text().await?;
        
        self.extract_streams(&html4)
    }

    pub async fn resolve_vidsrc_me(&self, tmdb_id: &str, media_type: &str, season: u32, episode: u32) -> Result<Vec<String>> {
        let url = if media_type == "movie" {
            format!("https://vidsrcme.ru/embed/movie?tmdb={}", tmdb_id)
        } else {
            format!("https://vidsrcme.ru/embed/tv?tmdb={}&season={}&episode={}", tmdb_id, season, episode)
        };

        let html = self.client.get(&url).send().await?.text().await?;
        
        let rcp_re = Regex::new(r#"src=["'](//cloudnestra\.com/rcp/[^"']+)["']"#)?;
        let rcp_url = match rcp_re.captures(&html) {
            Some(cap) => format!("https:{}", cap.get(1).unwrap().as_str()),
            None => return Ok(vec![]),
        };

        let html2 = self.client.get(&rcp_url).header(REFERER, &url).send().await?.text().await?;
        
        let prorcp_re = Regex::new(r"(/prorcp/[a-zA-Z0-9_=-]+)")?;
        let url3 = if let Some(cap) = prorcp_re.captures(&html2) {
            format!("https://cloudnestra.com{}", cap.get(1).unwrap().as_str())
        } else {
            let loc_re = Regex::new(r#"window\.location\.href\s*=\s*['"]([^'"]+)['"]"#)?;
            match loc_re.captures(&html2) {
                Some(cap) => {
                    let u = cap.get(1).unwrap().as_str();
                    if u.starts_with("/") { format!("https://cloudnestra.com{}", u) } else { u.to_string() }
                },
                None => return Ok(vec![]),
            }
        };

        let html3 = self.client.get(&url3).header(REFERER, &rcp_url).send().await?.text().await?;
        
        self.extract_streams(&html3)
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
