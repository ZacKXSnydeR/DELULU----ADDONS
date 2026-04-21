//! Persistent state management for MotherBox.
//! Stores tokens, UUIDs, and daily-limit markers in a JSON file
//! next to the binary (or in %APPDATA% for installed addons).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// The persistent state stored on disk.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct State {
    #[serde(default)]
    pub moviebox_token: String,
    #[serde(default)]
    pub moviebox_uuid: String,
    /// ISO date string (YYYY-MM-DD) of when daily limit was hit
    #[serde(default)]
    pub daily_limit_date: String,
    /// TMDB API key (optional override)
    #[serde(default)]
    pub tmdb_api_key: String,
}

impl State {
    /// Check if the daily limit was already hit today.
    pub fn is_daily_limit_hit(&self) -> bool {
        if self.daily_limit_date.is_empty() {
            return false;
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.daily_limit_date == today
    }

    /// Mark the daily limit as hit for today.
    pub fn mark_daily_limit(&mut self) {
        self.daily_limit_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        eprintln!("[state] Daily limit marked for {}", self.daily_limit_date);
    }

    /// Check if credentials are present.
    pub fn has_credentials(&self) -> bool {
        !self.moviebox_token.is_empty() && !self.moviebox_uuid.is_empty()
    }

    /// Update credentials.
    pub fn set_credentials(&mut self, token: String, uuid: String) {
        self.moviebox_token = token;
        self.moviebox_uuid = uuid;
    }
}

/// Determine the state file path.
/// Priority: 1) Next to the binary  2) %APPDATA%/com.delulu.desktop/addon_manager/  3) Current dir
fn state_file_path() -> PathBuf {
    // 1. Next to the binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("motherbox_state.json");
            if dir.exists() {
                return p;
            }
        }
    }

    // 2. %APPDATA% (Windows)
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let dir = PathBuf::from(appdata)
            .join("com.delulu.desktop")
            .join("addon_manager")
            .join("addons")
            .join("motherbox");
        if dir.exists() || fs::create_dir_all(&dir).is_ok() {
            return dir.join("motherbox_state.json");
        }
    }

    // 3. Current directory
    PathBuf::from("motherbox_state.json")
}

/// Load state from disk. Returns default State if file doesn't exist.
pub fn load_state() -> State {
    let path = state_file_path();
    eprintln!("[state] Loading from: {}", path.display());

    match fs::read_to_string(&path) {
        Ok(content) => {
            serde_json::from_str(&content).unwrap_or_else(|e| {
                eprintln!("[state] Parse error (using defaults): {}", e);
                State::default()
            })
        }
        Err(_) => {
            eprintln!("[state] No state file found — starting fresh");
            State::default()
        }
    }
}

/// Save state to disk.
pub fn save_state(state: &State) {
    let path = state_file_path();
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, &json) {
                eprintln!("[state] Failed to write state: {}", e);
            } else {
                eprintln!("[state] Saved to: {}", path.display());
            }
        }
        Err(e) => eprintln!("[state] Serialization error: {}", e),
    }
}

/// Also try to load credentials from environment variables (.env fallback).
/// This supports the legacy workflow where tokens are stored in .env.
pub fn load_state_with_env_fallback() -> State {
    let mut state = load_state();

    if state.moviebox_token.is_empty() {
        if let Ok(t) = std::env::var("MOVIEBOX_TOKEN") {
            if !t.is_empty() {
                eprintln!("[state] Using MOVIEBOX_TOKEN from environment");
                state.moviebox_token = t;
            }
        }
    }
    if state.moviebox_uuid.is_empty() {
        if let Ok(u) = std::env::var("MOVIEBOX_UUID") {
            if !u.is_empty() {
                eprintln!("[state] Using MOVIEBOX_UUID from environment");
                state.moviebox_uuid = u;
            }
        }
    }
    if state.tmdb_api_key.is_empty() {
        if let Ok(k) = std::env::var("TMDB_API_KEY") {
            if !k.is_empty() {
                state.tmdb_api_key = k;
            }
        }
    }

    state
}
