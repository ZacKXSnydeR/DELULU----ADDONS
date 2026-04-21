//! Browser-free authentication for MovieBox.
//! Harvests guest credentials via pure HTTP — no Chrome/Brave/WebSocket needed.
//!
//! Discovery: the PLAYER_HOST play endpoint automatically issues a fresh JWT token
//! via Set-Cookie when hit with any request (even with dummy params). This token
//! is valid for 90 days and grants 999 free streams per session.
//!
//! Strategy:
//!   1. Generate a random UUID v4
//!   2. Hit PLAYER_HOST/wefeed-h5api-bff/subject/play with uuid cookie
//!   3. Read the `Set-Cookie: token=...` from the response
//!   4. Persist both token + uuid to state.json

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::Rng;
use reqwest::Client;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::state;

pub const MOVIEBOX_HOST: &str = "https://moviebox.pk";
pub const PLAYER_HOST: &str = "https://123movienow.cc";
pub const UA_MOBILE: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Mobile Safari/537.36";

/// Decoded auth credentials.
#[derive(Debug, Clone)]
pub struct Auth {
    pub token: String,
    pub uuid: String,
}

/// Check if a JWT token is expired by decoding the payload.
pub fn is_token_expired(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return true;
    }

    let payload = match URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(p) => p,
        Err(_) => {
            // Try with standard padding
            let padded = format!("{}==", parts[1]);
            match URL_SAFE_NO_PAD.decode(&padded) {
                Ok(p) => p,
                Err(_) => return true,
            }
        }
    };

    let json: Value = match serde_json::from_slice(&payload) {
        Ok(j) => j,
        Err(_) => return true,
    };

    let exp = json["exp"].as_u64().unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    now > exp
}

/// Generate a random UUID v4 string.
fn random_uuid() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6] & 0x0f, bytes[7],
        (bytes[8] & 0x3f) | 0x80, bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Pure-HTTP credential harvest — fully autonomous, zero browser dependency.
///
/// Hits the play endpoint on PLAYER_HOST with a generated UUID. The server
/// auto-issues a fresh JWT token (90-day expiry, 999 free streams) via Set-Cookie.
pub async fn harvest_credentials_http(_client: &Client) -> Option<Auth> {
    eprintln!("[auth] Harvesting fresh credentials via play-endpoint probe...");

    let our_uuid = random_uuid();
    eprintln!("[auth] Generated UUID: {}", our_uuid);

    // Hit the play endpoint with a dummy request — the server issues a token
    // regardless of whether the subjectId is valid.
    let url = format!(
        "{}/wefeed-h5api-bff/subject/play?subjectId=0&se=0&ep=0&detailPath=init",
        PLAYER_HOST
    );

    // Build a fresh client WITHOUT cookie store so we can read raw Set-Cookie headers.
    // The shared client's cookie store would eat the cookies before we can extract them.
    let probe_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .gzip(true)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;

    match probe_client
        .get(&url)
        .header("User-Agent", UA_MOBILE)
        .header("Accept", "application/json")
        .header("Cookie", format!("uuid={}", our_uuid))
        .header("Referer", format!("{}/", PLAYER_HOST))
        .header("Origin", PLAYER_HOST)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            eprintln!("[auth] Probe response: {}", status);

            // Extract token from Set-Cookie header
            let mut token = None;
            for cookie_val in resp.headers().get_all("set-cookie").iter() {
                if let Ok(s) = cookie_val.to_str() {
                    if s.starts_with("token=") {
                        token = s.strip_prefix("token=")
                            .and_then(|v| v.split(';').next())
                            .map(|v| v.to_string());
                    }
                }
            }

            if let Some(t) = token {
                if !t.is_empty() && !is_token_expired(&t) {
                    eprintln!("[auth] [OK] Got fresh token via play-endpoint probe (90-day expiry)");
                    return Some(Auth {
                        token: t,
                        uuid: our_uuid,
                    });
                }
                eprintln!("[auth] Token received but appears expired or empty");
            } else {
                eprintln!("[auth] No token in Set-Cookie headers");

                // Fallback: try reading the response body for token info
                if let Ok(text) = resp.text().await {
                    eprintln!("[auth] Response body: {}", &text[..text.len().min(200)]);
                }
            }
        }
        Err(e) => {
            eprintln!("[auth] Play-endpoint probe failed: {}", e);
        }
    }

    eprintln!("[auth] [FAIL] Could not harvest credentials");
    None
}

/// Ensure we have valid credentials:
///   1. Load from state (disk + env)
///   2. If missing or expired → harvest fresh ones via play endpoint
///   3. Save to state
pub async fn ensure_auth(client: &Client) -> Option<Auth> {
    let mut st = state::load_state_with_env_fallback();

    // Check existing credentials
    if st.has_credentials() && !is_token_expired(&st.moviebox_token) {
        eprintln!("[auth] Using cached credentials (token not expired)");
        return Some(Auth {
            token: st.moviebox_token.clone(),
            uuid: st.moviebox_uuid.clone(),
        });
    }

    let reason = if !st.has_credentials() {
        "no credentials found"
    } else {
        "token expired"
    };
    eprintln!("[auth] Need fresh credentials ({})", reason);

    // Harvest new credentials — fully autonomous, no browser needed
    let auth = harvest_credentials_http(client).await?;

    // Persist for future runs
    st.set_credentials(auth.token.clone(), auth.uuid.clone());
    state::save_state(&st);

    Some(auth)
}

/// Force re-harvest (used after 401 errors).
pub async fn force_refresh(client: &Client) -> Option<Auth> {
    eprintln!("[auth] Forcing credential refresh...");
    let auth = harvest_credentials_http(client).await?;

    let mut st = state::load_state();
    st.set_credentials(auth.token.clone(), auth.uuid.clone());
    state::save_state(&st);

    Some(auth)
}
