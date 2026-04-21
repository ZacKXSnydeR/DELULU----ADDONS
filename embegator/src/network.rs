use anyhow::{bail, Context, Result};
use rquest::{header, Client};

use crate::models::{ApiResponse, MediaQuery, MediaType, Output};

// ── Token generation ─────────────────────────────────────────────────────────

/// Executes the headless WASM bypass shim in Node.js to generate the Vidlink authentication token.
pub async fn generate_api_token(media_id: &str, bypass_path: &Option<String>) -> Result<String> {
    // Implement centralized path resolver integration
    let bypass_script_path = if let Some(p) = bypass_path {
        let path = std::path::PathBuf::from(p);
        if !path.exists() {
            bail!("Injected bypass path does not exist: {:?}", path);
        }
        path
    } else {
        // Fallback for independent CLI testing
        let exe_path =
            std::env::current_exe().context("Failed to determine current executable path")?;

        let fallback = exe_path
            .parent()
            .context("Executable has no parent directory")?
            .join("bypass/bypass.js");

        if !fallback.exists() {
            bail!("Bypass script not found at fallback path: {:?}", fallback);
        }
        fallback
    };

    let node_exe_path = bypass_script_path
        .parent()
        .context("Bypass script has no parent directory")?
        .join("node.exe");

    let node_cmd = if node_exe_path.exists() {
        node_exe_path.to_string_lossy().into_owned()
    } else {
        "node".to_string()
    };

    let mut command = tokio::process::Command::new(&node_cmd);
    command.args([bypass_script_path.to_str().unwrap(), media_id]);

    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let output = command.output().await.context(format!(
        "Failed to spawn node bypass.js at {:?}",
        bypass_script_path
    ))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Node bypass script failed with status: {}. Stderr: {}",
            output.status,
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let token = stdout.trim().to_string();

    if token.is_empty() {
        bail!("Node bypass script returned an empty token.");
    }

    // We expect a robust 72 UUID/hash. If it's vastly different, things might have changed.
    if token.len() < 50 {
        bail!("Unexpected bypass token format or length: {}", token.len());
    }

    Ok(token)
}

// ── HTTP client ──────────────────────────────────────────────────────────────

fn build_client() -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::ACCEPT, "application/json, */*;q=0.8".parse()?);
    headers.insert(header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse()?);
    headers.insert(header::ACCEPT_ENCODING, "gzip, deflate, br".parse()?);
    headers.insert(
        "Sec-Ch-Ua",
        r#""Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99""#.parse()?,
    );
    headers.insert("Sec-Ch-Ua-Mobile", "?0".parse()?);
    headers.insert("Sec-Ch-Ua-Platform", r#""Windows""#.parse()?);
    headers.insert("Sec-Fetch-Dest", "empty".parse()?);
    headers.insert("Sec-Fetch-Mode", "cors".parse()?);
    headers.insert("Sec-Fetch-Site", "same-origin".parse()?);
    headers.insert("DNT", "1".parse()?);

    let client = Client::builder()
        .emulation(rquest_util::Emulation::Chrome124)
        .default_headers(headers)
        .gzip(true)
        .brotli(true)
        .build()?;
    Ok(client)
}

// ── Public fetch entry-point ─────────────────────────────────────────────────

pub async fn fetch_media(query: MediaQuery) -> Result<Output> {
    let token = generate_api_token(&query.tmdb_id, &query.bypass_path).await?;

    let url = build_url(&query, &token)?;

    let client = build_client()?;

    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status} from {url}");
    }

    let raw = resp.text().await.context("reading response body")?;

    let parsed: ApiResponse = serde_json::from_str(&raw)
        .with_context(|| format!("JSON parse error; body was:\n{raw}"))?;

    Ok(Output::from_response(parsed))
}

fn build_url(query: &MediaQuery, token: &str) -> Result<String> {
    match query.media_type {
        MediaType::Movie => Ok(format!("https://vidlink.pro/api/b/movie/{token}")),
        MediaType::TvShow | MediaType::Anime => {
            let s = query.season.context("season required for TV/Anime")?;
            let e = query.episode.context("episode required for TV/Anime")?;
            Ok(format!("https://vidlink.pro/api/b/tv/{token}/{s}/{e}"))
        }
    }
}
