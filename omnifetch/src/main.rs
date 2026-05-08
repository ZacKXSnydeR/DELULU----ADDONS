use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

// ─── Constants ───────────────────────────────────────────────────────────────

const YTDLP_DOWNLOAD_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YTDLP_UPDATE_INTERVAL_SECS: u64 = 7 * 24 * 3600; // 7 days
const VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── RPC Models ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RpcRequest {
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct RpcResponse {
    id: Value,
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

// ─── Entry Point ─────────────────────────────────────────────────────────────

fn main() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout_lock = Arc::new(Mutex::new(io::stdout()));
    let mut line = String::new();

    // Multi-threaded RPC loop
    while let Ok(bytes) = reader.read_line(&mut line) {
        if bytes == 0 {
            break;
        }

        let trimmed = line.trim().to_string();
        line.clear();

        if trimmed.is_empty() {
            continue;
        }

        let stdout_clone = Arc::clone(&stdout_lock);

        thread::spawn(move || {
            if let Ok(req) = serde_json::from_str::<RpcRequest>(&trimmed) {
                let id = req.id.clone().unwrap_or(json!(null));
                
                // Route request
                let res = handle_request(&id, &req.method, req.params, stdout_clone.clone());

                // Build response
                let response = match res {
                    Ok(result) => RpcResponse {
                        id,
                        jsonrpc: "2.0".to_string(),
                        result: Some(result),
                        error: None,
                    },
                    Err(err) => RpcResponse {
                        id,
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(json!({ "code": -32603, "message": err })),
                    },
                };

                // Thread-safe output
                if let Ok(out) = serde_json::to_string(&response) {
                    let mut lock = stdout_clone.lock().unwrap();
                    let _ = writeln!(lock, "{}", out);
                    let _ = lock.flush();
                }
            }
        });
    }
}

// ─── Request Router ──────────────────────────────────────────────────────────

fn handle_request(
    req_id: &Value,
    method: &str,
    params: Option<Value>,
    stdout_lock: Arc<Mutex<io::Stdout>>,
) -> Result<Value, String> {
    match method {
        "healthCheck" => {
            let yt_ready = ytdlp_path().exists();
            Ok(json!({
                "ok": true,
                "version": VERSION,
                "ytdlpReady": yt_ready,
                "capabilities": ["resolveTrailer", "download", "updateYtdlp", "setup"],
                "supportsConcurrency": true
            }))
        }

        "resolveTrailer" => {
            let p = params.ok_or("Missing params")?;
            let yt_id = p
                .get("ytId")
                .and_then(|v| v.as_str())
                .ok_or("Missing ytId in params")?;
            resolve_trailer(yt_id)
        }

        "download" => {
            let p = params.ok_or("Missing params")?;
            let url = p
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("Missing url in params")?;
            let output_dir = p
                .get("outputDir")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let filename = p.get("filename").and_then(|v| v.as_str());
            let format = p.get("format").and_then(|v| v.as_str());
            let headers: Option<HashMap<String, String>> = p
                .get("headers")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let subtitles = p
                .get("subtitles")
                .and_then(|v| v.as_array())
                .cloned();

            download_media(
                req_id,
                url,
                output_dir,
                filename,
                format,
                headers.as_ref(),
                subtitles.as_deref(),
                stdout_lock,
            )
        }

        "setup" => {
            let path = ytdlp_path();
            if path.exists() {
                Ok(json!({
                    "ok": true,
                    "alreadyReady": true,
                    "message": "yt-dlp already present"
                }))
            } else {
                download_ytdlp_binary(&path)?;
                Ok(json!({
                    "ok": true,
                    "alreadyReady": false,
                    "message": "yt-dlp downloaded successfully"
                }))
            }
        }

        "updateYtdlp" => {
            force_update_ytdlp()?;
            Ok(json!({
                "ok": true,
                "message": "yt-dlp updated to latest version"
            }))
        }

        _ => Err(format!("Method '{}' not supported", method)),
    }
}

// ─── yt-dlp Binary Management ────────────────────────────────────────────────

fn ytdlp_path() -> PathBuf {
    let exe_dir = env::current_exe()
        .map(|p| p.parent().unwrap().to_path_buf())
        .unwrap_or_else(|_| env::current_dir().unwrap());
    exe_dir.join("yt-dlp.exe")
}

fn ensure_ytdlp() -> Result<PathBuf, String> {
    let path = ytdlp_path();

    let needs_download = if !path.exists() {
        eprintln!("[media-engine] yt-dlp.exe not found, downloading...");
        true
    } else {
        if let Ok(meta) = fs::metadata(&path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(age) = SystemTime::now().duration_since(modified) {
                    if age.as_secs() > YTDLP_UPDATE_INTERVAL_SECS {
                        eprintln!("[media-engine] yt-dlp.exe is {} days old, updating...", age.as_secs() / 86400);
                        true
                    } else {
                        false
                    }
                } else { false }
            } else { false }
        } else { false }
    };

    if needs_download {
        download_ytdlp_binary(&path)?;
    }

    Ok(path)
}

fn download_ytdlp_binary(dest: &PathBuf) -> Result<(), String> {
    eprintln!("[media-engine] Downloading yt-dlp from GitHub releases...");

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?
        .get(YTDLP_DOWNLOAD_URL)
        .send()
        .map_err(|e| format!("Download request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download failed with status: {}", response.status()));
    }

    let bytes = response
        .bytes()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let tmp = dest.with_extension("exe.tmp");
    fs::write(&tmp, &bytes).map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp, dest).map_err(|e| format!("Failed to rename: {}", e))?;

    eprintln!("[media-engine] yt-dlp.exe updated ({:.1} MB)", bytes.len() as f64 / 1_048_576.0);
    Ok(())
}

fn force_update_ytdlp() -> Result<(), String> {
    let path = ytdlp_path();
    download_ytdlp_binary(&path)
}

// ─── Spawn Helper (no terminal ghosting) ─────────────────────────────────────

#[cfg(target_os = "windows")]
fn command_hidden(cmd: &PathBuf, args: &[&str]) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut c = Command::new(cmd);
    c.args(args).creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(target_os = "windows"))]
fn command_hidden(cmd: &PathBuf, args: &[&str]) -> Command {
    let mut c = Command::new(cmd);
    c.args(args);
    c
}

// ─── resolveTrailer ──────────────────────────────────────────────────────────

fn resolve_trailer(yt_id: &str) -> Result<Value, String> {
    let ytdlp = ensure_ytdlp()?;
    let url = format!("https://www.youtube.com/watch?v={}", yt_id);

    let output = command_hidden(&ytdlp, &["-j", "--no-playlist", &url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to execute yt-dlp: {}", e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp error: {}", err));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: Value =
        serde_json::from_str(&json_str).map_err(|e| format!("Invalid JSON from yt-dlp: {}", e))?;

    let formats = parsed
        .get("formats")
        .and_then(|v| v.as_array())
        .ok_or("No formats found in yt-dlp output")?;

    // ── Best audio (single) ──
    let mut best_audio: Option<String> = None;
    let mut best_audio_abr: f64 = 0.0;

    for fmt in formats {
        let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
        let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
        let ext = fmt.get("ext").and_then(|v| v.as_str()).unwrap_or("");

        if acodec != "none" && vcodec == "none" && (ext == "m4a" || ext == "webm") {
            let abr = fmt.get("abr").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if abr > best_audio_abr {
                best_audio_abr = abr;
                if let Some(u) = fmt.get("url").and_then(|v| v.as_str()) {
                    best_audio = Some(u.to_string());
                }
            }
        }
    }

    // ── Best video per resolution (DASH, >=720p) ──
    let mut videos_by_res: HashMap<i64, (String, f64)> = HashMap::new();

    for fmt in formats {
        let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
        let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
        let ext = fmt.get("ext").and_then(|v| v.as_str()).unwrap_or("");
        let height = fmt.get("height").and_then(|v| v.as_i64()).unwrap_or(0);

        if height < 720 || vcodec == "none" || acodec != "none" {
            continue;
        }
        if ext != "mp4" && ext != "webm" {
            continue;
        }

        let url = match fmt.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => continue,
        };

        let tbr = fmt.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let existing_tbr = videos_by_res.get(&height).map(|x| x.1).unwrap_or(0.0);
        if tbr > existing_tbr {
            videos_by_res.insert(height, (url, tbr));
        }
    }

    // ── Build streams array (highest first) ──
    let mut heights: Vec<i64> = videos_by_res.keys().copied().collect();
    heights.sort_by(|a, b| b.cmp(a));

    let mut streams = Vec::new();
    for h in heights {
        if let Some((vid_url, _)) = videos_by_res.get(&h) {
            streams.push(json!({
                "resolution": format!("{}p", h),
                "videoUrl": vid_url,
                "audioUrl": best_audio.clone(),
            }));
        }
    }

    // ── Fallback: pre-muxed (audio+video combined) ──
    if streams.is_empty() {
        for fmt in formats {
            let vcodec = fmt.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
            let acodec = fmt.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
            let height = fmt.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
            if vcodec != "none" && acodec != "none" && height >= 720 {
                if let Some(u) = fmt.get("url").and_then(|v| v.as_str()) {
                    streams.push(json!({
                        "resolution": format!("{}p", height),
                        "videoUrl": u,
                        "audioUrl": null,
                    }));
                }
            }
        }
    }

    if streams.is_empty() {
        return Err("No suitable formats (>=720p) found".to_string());
    }

    Ok(json!({
        "success": true,
        "streams": streams
    }))
}

// ─── Progress Parser ─────────────────────────────────────────────────────────

fn parse_progress(line: &str) -> Option<Value> {
    if line.starts_with("[download]") && line.contains('%') {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let mut percent = None;
        let mut speed = None;
        let mut eta = None;
        let mut total_size = None;

        for (i, p) in parts.iter().enumerate() {
            if p.ends_with('%') {
                percent = p.trim_end_matches('%').parse::<f64>().ok();
            } else if p.contains("iB/s") || p.contains("B/s") {
                speed = Some(p.to_string());
            } else if *p == "ETA" && i + 1 < parts.len() {
                eta = Some(parts[i + 1].to_string());
            } else if (p.contains("iB") || p.contains("B")) 
                && !p.contains("/s") 
                && total_size.is_none() 
                && !p.ends_with('%') 
            {
                total_size = Some(p.to_string());
            }
        }

        if let Some(pct) = percent {
            return Some(json!({
                "percent": pct,
                "speed": speed.unwrap_or_default(),
                "eta": eta.unwrap_or_default(),
                "size": total_size.unwrap_or_default()
            }));
        }
    }
    None
}

// ─── download ────────────────────────────────────────────────────────────────

fn download_media(
    req_id: &Value,
    url: &str,
    output_dir: &str,
    filename: Option<&str>,
    format: Option<&str>,
    headers: Option<&HashMap<String, String>>,
    subtitles: Option<&[Value]>,
    stdout_lock: Arc<Mutex<io::Stdout>>,
) -> Result<Value, String> {
    let ytdlp = ensure_ytdlp()?;

    // Build output template
    let output_template = if let Some(name) = filename {
        format!("{}/{}", output_dir, name)
    } else {
        format!("{}/%(title)s.%(ext)s", output_dir)
    };

    let mut args: Vec<String> = vec![
        url.to_string(),
        "-o".to_string(),
        output_template.clone(),
        "--no-playlist".to_string(),
        "--progress".to_string(),
        "--newline".to_string(), // important for parsing!
    ];

    if let Some(fmt) = format {
        args.push("-f".to_string());
        args.push(fmt.to_string());
    }

    if let Some(hdrs) = headers {
        if let Some(referer) = hdrs.get("Referer").or(hdrs.get("referer")) {
            args.push("--referer".to_string());
            args.push(referer.clone());
        }
        if let Some(origin) = hdrs.get("Origin").or(hdrs.get("origin")) {
            args.push("--add-header".to_string());
            args.push(format!("Origin: {}", origin));
        }
        for (k, v) in hdrs {
            let kl = k.to_lowercase();
            if kl != "referer" && kl != "origin" {
                args.push("--add-header".to_string());
                args.push(format!("{}: {}", k, v));
            }
        }
    }

    if let Some(subs) = subtitles {
        for sub in subs {
            if let (Some(sub_url), Some(lang)) = (
                sub.get("url").and_then(|v| v.as_str()),
                sub.get("language").and_then(|v| v.as_str()),
            ) {
                let sub_filename = format!(
                    "{}/{}.{}.vtt",
                    output_dir,
                    filename.unwrap_or("video"),
                    lang
                );
                if let Ok(resp) = reqwest::blocking::get(sub_url) {
                    if let Ok(body) = resp.bytes() {
                        let _ = fs::write(&sub_filename, &body);
                        eprintln!("[media-engine] Saved subtitle: {}", sub_filename);
                    }
                }
            }
        }
    }

    let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // Spawn yt-dlp with piped stdout
    let mut child = command_hidden(&ytdlp, &args_str)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // Let stderr pass through to addon's stderr
        .spawn()
        .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?;

    let child_stdout = child.stdout.take().expect("Failed to grab stdout");
    let reader = BufReader::new(child_stdout);

    let mut final_path = output_template.clone();

    for line in reader.lines() {
        if let Ok(l) = line {
            // Check for progress
            if let Some(prog) = parse_progress(&l) {
                let event = json!({
                    "jsonrpc": "2.0",
                    "method": "downloadProgress",
                    "params": {
                        "taskId": req_id,
                        "progress": prog
                    }
                });
                
                // Thread-safe progress emission
                if let Ok(out) = serde_json::to_string(&event) {
                    let mut lock = stdout_lock.lock().unwrap();
                    let _ = writeln!(lock, "{}", out);
                    let _ = lock.flush();
                }
            } else if l.contains("[Merger]") || l.contains("Destination:") || l.contains("[download]") {
                if let Some(dest) = l.split("Destination: ").nth(1) {
                    final_path = dest.trim().to_string();
                } else if let Some(dest) = l.strip_prefix("[Merger] Merging formats into \"") {
                    if let Some(path) = dest.strip_suffix('"') {
                        final_path = path.to_string();
                    }
                }
            }
        }
    }

    let status = child.wait().map_err(|e| format!("Failed to wait for yt-dlp: {}", e))?;
    if !status.success() {
        return Err(format!("Download failed (exit code: {})", status.code().unwrap_or(-1)));
    }

    Ok(json!({
        "success": true,
        "outputPath": final_path,
        "message": "Download complete"
    }))
}
