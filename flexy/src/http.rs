use wreq::Client;
use std::time::Duration;

pub fn build_client() -> Client {
    Client::builder()
        .emulation(wreq_util::Emulation::Chrome120)
        .timeout(Duration::from_secs(20))
        .redirect(wreq::redirect::Policy::limited(10))
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
