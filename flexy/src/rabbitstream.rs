use reqwest::Client;
use serde_json::{json, Value};
use p256::ecdsa::{SigningKey, signature::Signer};
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use aes_gcm::{Aes256Gcm, Nonce, aead::{Aead, KeyInit}};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use uuid::Uuid;
use url::Url;

use crate::models::{StreamResult, Subtitle};

fn encode_b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_b64url(s: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut s = s.replace("-", "+").replace("_", "/");
    let padding = s.len() % 4;
    if padding > 0 {
        s.push_str(&"=".repeat(4 - padding));
    }
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}

pub async fn extract(
    client: &Client,
    embed_url: &str,
) -> Result<StreamResult, Box<dyn std::error::Error + Send + Sync>> {
    let url_parsed = Url::parse(embed_url)?;
    let base_api = format!("{}://{}", url_parsed.scheme(), url_parsed.host_str().unwrap());
    
    let re = regex::Regex::new(r"/e/([a-zA-Z0-9_-]+)").unwrap();
    let video_id = match re.captures(embed_url) {
        Some(cap) => cap[1].to_string(),
        None => return Err("Could not find video ID in embed URL".into()),
    };

    let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
    let referer = embed_url;

    // 1. Challenge
    let challenge_url = format!("{}/api/videos/access/challenge", base_api);
    let r1 = client.post(&challenge_url)
        .header("User-Agent", ua)
        .header("Referer", referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Content-Type", "application/json")
        .send()
        .await?;
        
    if !r1.status().is_success() {
        return Err(format!("Challenge failed: {}", r1.status()).into());
    }
    let challenge_data: Value = r1.json().await?;
    
    let nonce = challenge_data["nonce"].as_str().unwrap_or("");
    let challenge_id = challenge_data["challenge_id"].as_str().unwrap_or("");

    // 2. Attest
    let secret_key = SecretKey::random(&mut rand_core::OsRng);
    let signing_key = SigningKey::from(&secret_key);
    
    let signature: ecdsa::Signature<p256::NistP256> = signing_key.sign(nonce.as_bytes());
    let sig_bytes = signature.to_bytes();
    let sig_b64 = encode_b64url(&sig_bytes);

    let pub_key = secret_key.public_key();
    let pub_key_pt = pub_key.to_encoded_point(false);
    
    let jwk = json!({
        "crv": "P-256",
        "ext": true,
        "key_ops": ["verify"],
        "kty": "EC",
        "x": encode_b64url(pub_key_pt.x().unwrap()),
        "y": encode_b64url(pub_key_pt.y().unwrap())
    });

    let client_payload = json!({
        "user_agent": ua,
        "architecture": "x86",
        "bitness": "64",
        "platform": "Windows",
        "platform_version": "10.0.0",
        "model": "",
        "ua_full_version": "124.0.0.0",
        "brand_full_versions": [{"brand": "Chromium", "version": "124.0.0.0"}],
        "pixel_ratio": 1,
        "screen_width": 1920,
        "screen_height": 1080,
        "color_depth": 24,
        "languages": ["en-US"],
        "timezone": "UTC",
        "hardware_concurrency": 8,
        "device_memory": 8,
        "touch_points": 0,
        "webgl_vendor": "Google Inc. (Google)",
        "webgl_renderer": "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
        "canvas_hash": "_xjcrc8La-Vnxpr6a6vNFOOdnRcHHQ0tzgT_V3atRqo",
        "audio_hash": "RyBmlOc4cA7XhqmvkyO40eo8sOa5q-CFlrTnf70qADY",
        "pointer_type": "fine,hover",
        "extra": {"vendor": "Google Inc."}
    });

    let viewer_id = Uuid::new_v4().simple().to_string();
    let device_id = Uuid::new_v4().simple().to_string();

    let attest_payload = json!({
        "viewer_id": viewer_id,
        "device_id": device_id,
        "challenge_id": challenge_id,
        "nonce": nonce,
        "signature": sig_b64,
        "public_key": jwk,
        "client": client_payload,
        "storage": {},
        "attributes": {"entropy": "high"}
    });

    let attest_url = format!("{}/api/videos/access/attest", base_api);
    let r2 = client.post(&attest_url)
        .header("User-Agent", ua)
        .header("Referer", referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Content-Type", "application/json")
        .json(&attest_payload)
        .send()
        .await?;

    if !r2.status().is_success() {
        return Err(format!("Attest failed: {}", r2.status()).into());
    }
    let attest_data: Value = r2.json().await?;
    let token = attest_data["token"].as_str().unwrap_or("");
    let conf = attest_data["confidence"].as_f64().unwrap_or(0.0);

    // 3. Playback
    let playback_payload = json!({
        "fingerprint": {
            "token": token,
            "viewer_id": viewer_id,
            "device_id": device_id,
            "confidence": conf
        }
    });

    let playback_url = format!("{}/api/videos/{}/embed/playback", base_api, video_id);
    let r3 = client.post(&playback_url)
        .header("User-Agent", ua)
        .header("Referer", referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", token))
        .json(&playback_payload)
        .send()
        .await?;

    if !r3.status().is_success() {
        return Err(format!("Playback failed: {}", r3.status()).into());
    }
    let playback_json: Value = r3.json().await?;
    let playback_data = &playback_json["playback"];
    if playback_data.is_null() {
        return Err("No playback data in response".into());
    }

    // 4. Decrypt
    let mut key_bytes = Vec::new();
    if let Some(parts) = playback_data["key_parts"].as_array() {
        for part in parts {
            if let Some(s) = part.as_str() {
                key_bytes.extend(decode_b64url(s)?);
            }
        }
    } else {
        return Err("No key_parts array".into());
    }

    let iv = decode_b64url(playback_data["iv"].as_str().unwrap_or(""))?;
    let payload = decode_b64url(playback_data["payload"].as_str().unwrap_or(""))?;

    let cipher = Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("Invalid key length: {:?}", e))?;
    let nonce_gcm = Nonce::from_slice(&iv);
    
    let decrypted = match cipher.decrypt(nonce_gcm, payload.as_ref()) {
        Ok(d) => d,
        Err(e) => return Err(format!("AES-GCM decryption failed: {:?}", e).into()),
    };

    let decrypted_str = String::from_utf8(decrypted)?;
    let parsed: Value = serde_json::from_str(&decrypted_str)?;

    let mut master_url = None;
    if let Some(sources) = parsed["sources"].as_array() {
        if let Some(first) = sources.first() {
            master_url = first["url"].as_str().map(|s| s.to_string());
        }
    }

    let mut subtitles = Vec::new();
    if let Some(tracks) = parsed["tracks"].as_array() {
        for track in tracks {
            if track["kind"].as_str() == Some("captions") {
                if let (Some(url), Some(lang)) = (track["file"].as_str(), track["label"].as_str()) {
                    subtitles.push(Subtitle {
                        url: url.to_string(),
                        language: lang.to_string(),
                    });
                }
            }
        }
    }

    if let Some(url) = master_url {
        // Prepare headers (Delulu requirements)
        let hdrs = crate::models::StreamHeaders {
            referer: Some(base_api.clone()),
            origin: Some(base_api),
            user_agent: Some(ua.to_string()),
        };

        Ok(StreamResult {
            success: true,
            stream_url: Some(url),
            headers: Some(hdrs),
            subtitles: Some(subtitles),
            ..Default::default()
        })
    } else {
        Err("No playable stream found in decrypted payload".into())
    }
}
