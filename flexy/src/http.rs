use reqwest::{Client, header};
use std::time::Duration;

pub fn build_client() -> Client {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36".parse().unwrap());
    headers.insert(header::ACCEPT, "application/json,text/html,*/*".parse().unwrap());
    headers.insert(header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());

    Client::builder()
        .default_headers(headers)
        .gzip(true)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(10))
        .cookie_store(true)
        .build()
        .expect("Failed to build HTTP client")
}

pub async fn http_get(client: &Client, url: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    for attempt in 0..2u8 {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_server_error() && attempt == 0 {
                    eprintln!("[http] 5xx on GET {} — retrying in 500ms", &url[..url.len().min(80)]);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                let body = resp.text().await?;
                if status.is_client_error() {
                    return Err(format!("HTTP {} for {}", status.as_u16(), &url[..url.len().min(80)]).into());
                }
                return Ok(body);
            }
            Err(e) => {
                if attempt == 0 {
                    eprintln!("[http] Connection error on GET {} — retrying: {}", &url[..url.len().min(60)], e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                return Err(e.into());
            }
        }
    }
    Err("http_get exhausted retries".into())
}
