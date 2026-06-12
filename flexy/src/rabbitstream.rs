use wreq::Client;
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

const BE: usize = 512;
const LT: usize = BE - 1;
const DR: usize = 2;
const LR: u32 = 2654435761;
const HR: u32 = 2246822519;

fn ye_fast(t: &mut [u32; 4]) {
    t[0] = t[0].wrapping_add(t[1]); t[3] = (t[3] ^ t[0]).rotate_left(16);
    t[2] = t[2].wrapping_add(t[3]); t[1] = (t[1] ^ t[2]).rotate_left(12);
    t[0] = t[0].wrapping_add(t[1]); t[3] = (t[3] ^ t[0]).rotate_left(8);
    t[2] = t[2].wrapping_add(t[3]); t[1] = (t[1] ^ t[2]).rotate_left(7);
}

fn gr(input: &[u8]) -> [u32; 8] {
    let mut e = [1779033703u32, 3144134277u32, 1013904242u32, 2773480762u32];
    for &b in input {
        e[0] = e[0].wrapping_add(b as u32);
        e[0] = e[0].rotate_left(7);
        ye_fast(&mut e);
    }
    for _ in 0..8 {
        ye_fast(&mut e);
    }
    let mut r = [0u32; BE];
    for i in 0..BE {
        ye_fast(&mut e);
        r[i] = e[0] ^ e[2];
    }
    for _ in 0..DR {
        for s in 0..BE {
            let a = (r[s] as usize) & LT;
            let mut c = r[s].wrapping_add(r[a]);
            c = c.rotate_left(13);
            let s_plus_1 = (s + 1) & LT;
            c = c ^ r[s_plus_1].wrapping_mul(LR);
            r[s] = c;
            e[0] = e[0] ^ c;
            ye_fast(&mut e);
        }
    }
    let mut n = [0u32; 8];
    let o = BE / 8;
    for i in 0..8 {
        ye_fast(&mut e);
        let mut s = e[0];
        let a = i * o;
        for c in 0..o {
            let d = r[a + c];
            s = s.wrapping_add(d);
            s = s.rotate_left(5);
            s = s ^ d.wrapping_mul(HR);
        }
        n[i] = s ^ e[2];
    }
    n
}

fn wr(t: &[u32; 8]) -> u32 {
    let mut e = 0;
    for &n in t {
        if n == 0 {
            e += 32;
        } else {
            return e + n.leading_zeros();
        }
    }
    e
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
        let status = r2.status();
        let body = r2.text().await.unwrap_or_default();
        return Err(format!("Attest failed: {} - {}", status, body).into());
    }
    let attest_data: Value = r2.json().await?;
    let token = attest_data["token"].as_str().unwrap_or("");
    let conf = attest_data["confidence"].as_f64().unwrap_or(0.0);
    
    let attest_viewer_id = attest_data["viewer_id"].as_str().unwrap_or("");
    let attest_device_id = attest_data["device_id"].as_str().unwrap_or("");

    // 3. Captcha
    let captcha_payload = json!({
        "fingerprint": {
            "token": token,
            "viewer_id": viewer_id,
            "device_id": device_id,
            "confidence": conf
        }
    });

    let session_id = uuid::Uuid::new_v4().to_string().replace("-", "");

    let captcha_url = format!("{}/api/videos/{}/embed/captcha", base_api, video_id);
    let r_captcha = client.post(&captcha_url)
        .header("User-Agent", ua)
        .header("Origin", base_api.clone())
        .header("Referer", referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-Playback-Session-Id", &session_id)
        .header("Content-Type", "application/json")
        .json(&captcha_payload)
        .send()
        .await?;

    if !r_captcha.status().is_success() {
        let status = r_captcha.status();
        let body = r_captcha.text().await.unwrap_or_default();
        return Err(format!("Captcha failed: {} - {}", status, body).into());
    }

    let captcha_data: Value = r_captcha.json().await?;
    
    let pow_nonce = captcha_data["pow_nonce"].as_str().unwrap_or("");
    let pow_difficulty = captcha_data["pow_difficulty"].as_u64().unwrap_or(2) as usize;
    let pow_token = captcha_data["pow_token"].as_str().unwrap_or("");
    
    let mut new_token = token.to_string();
    if !pow_nonce.is_empty() && !pow_token.is_empty() {
        let mut solution = 0u64;
        loop {
            let s_str = solution.to_string();
            let input_str = format!("{}:{}", pow_nonce, s_str);
            let d = gr(input_str.as_bytes());
            if wr(&d) >= pow_difficulty as u32 {
                break;
            }
            solution += 1;
        }

        let verify_payload = json!({
            "pow_token": pow_token,
            "solution": solution.to_string(),
            "fingerprint": {
                "token": token,
                "viewer_id": attest_viewer_id,
                "device_id": attest_device_id,
                "confidence": conf
            }
        });

        let verify_url = format!("{}/api/videos/{}/embed/captcha/verify", base_api, video_id);
        let r_verify = client.post(&verify_url)
            .header("User-Agent", ua)
            .header("Origin", base_api.clone())
            .header("Referer", referer)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-Playback-Session-Id", &session_id)
            .header("Content-Type", "application/json")
            .json(&verify_payload)
            .send()
            .await?;

        let verify_status = r_verify.status();
        let verify_body = r_verify.text().await.unwrap_or_default();

        if !verify_status.is_success() {
            return Err(format!("Verify failed: {} - {}", verify_status, verify_body).into());
        }

        if let Ok(verify_data) = serde_json::from_str::<Value>(&verify_body) {
            // try all known token field names
            if let Some(tok) = verify_data["token"].as_str() {
                new_token = tok.to_string();
            } else if let Some(tok) = verify_data["access_token"].as_str() {
                new_token = tok.to_string();
            } else if let Some(tok) = verify_data["access_token"]["token"].as_str() {
                new_token = tok.to_string();
            }
        }
    }

    // 4. Playback
    let playback_payload = json!({
        "fingerprint": {
            "token": token,
            "viewer_id": attest_viewer_id,
            "device_id": attest_device_id,
            "confidence": conf
        }
    });

    let playback_url = format!("{}/api/videos/{}/embed/playback", base_api, video_id);
    let mut pb_req = client.post(&playback_url)
        .header("User-Agent", ua)
        .header("Origin", base_api.clone())
        .header("Referer", referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-Playback-Session-Id", &session_id)
        .header("Content-Type", "application/json");

    // Clearance token from captcha verify goes as X-Captcha-Token header
    if new_token != token {
        pb_req = pb_req.header("X-Captcha-Token", &new_token);
    }

    let r3 = pb_req
        .json(&playback_payload)
        .send()
        .await?;

    if !r3.status().is_success() {
        let status = r3.status();
        let body = r3.text().await.unwrap_or_default();
        return Err(format!("Playback failed: {} - {}", status, body).into());
    }
    let playback_json: Value = r3.json().await?;
    let playback_data = &playback_json["playback"];
    if playback_data.is_null() {
        return Err(format!("No playback data in response: {}", playback_json).into());
    }

    // 4. Decrypt
    // The JS selects key_parts using Si(version, count) which maps version -> [idx_a, idx_b] (1-indexed)
    // xi() builds: for n in 1..=20: e[n] = [n^0, 31-n^0] = [n, 31-n]
    // Then ko concatenates the two selected parts to form the 32-byte AES-256 key
    let version_str = playback_data["version"].as_str().unwrap_or("");
    let key_parts = match playback_data["key_parts"].as_array() {
        Some(p) => p,
        None => return Err("No key_parts array".into()),
    };
    let key_count = key_parts.len();

    let key_bytes: Vec<u8> = if let Ok(v) = version_str.trim().parse::<usize>() {
        // xi(): idx_a = v ^ 0 = v, idx_b = 31 - v ^ 0 = 31 - v (1-indexed)
        let idx_a = v;
        let idx_b = 31usize.wrapping_sub(v);
        if idx_a >= 1 && idx_b >= 1 && idx_a <= key_count && idx_b <= key_count {
            // selected = [key_parts[idx_a-1], key_parts[idx_b-1]] in this order
            let part_a = key_parts[idx_a - 1].as_str().unwrap_or("");
            let part_b = key_parts[idx_b - 1].as_str().unwrap_or("");
            let mut kb = decode_b64url(part_a)?;
            kb.extend(decode_b64url(part_b)?);
            kb
        } else {
            // fallback: concatenate all
            let mut kb = Vec::new();
            for part in key_parts { if let Some(s) = part.as_str() { kb.extend(decode_b64url(s)?); } }
            kb
        }
    } else {
        // No version: concatenate all
        let mut kb = Vec::new();
        for part in key_parts { if let Some(s) = part.as_str() { kb.extend(decode_b64url(s)?); } }
        kb
    };

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
