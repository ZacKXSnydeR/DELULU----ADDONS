//! Embeds all bypass runtime files at compile-time and extracts them
//! to a cache directory next to the executable on first run.
//!
//! This makes `embegator.exe` a true single-file binary — no sidecar
//! `bypass/` folder needed.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

// ── Compile-time embedded assets ────────────────────────────────────────────

const BYPASS_JS: &[u8] = include_bytes!("../bypass/bypass.js");
const WASM_EXEC_JS: &[u8] = include_bytes!("../bypass/wasm_exec.js");
const FU_WASM: &[u8] = include_bytes!("../bypass/fu.wasm");
const LIBSODIUM_JS: &[u8] =
    include_bytes!("../bypass/node_modules/libsodium/dist/modules/libsodium.js");
const LIBSODIUM_PKG: &[u8] =
    include_bytes!("../bypass/node_modules/libsodium/package.json");
const LIBSODIUM_WRAPPERS_JS: &[u8] =
    include_bytes!("../bypass/node_modules/libsodium-wrappers/dist/modules/libsodium-wrappers.js");
const LIBSODIUM_WRAPPERS_PKG: &[u8] =
    include_bytes!("../bypass/node_modules/libsodium-wrappers/package.json");

/// A file to extract: (relative path from cache root, content bytes).
const ASSETS: &[(&str, &[u8])] = &[
    ("bypass.js", BYPASS_JS),
    ("wasm_exec.js", WASM_EXEC_JS),
    ("fu.wasm", FU_WASM),
    (
        "node_modules/libsodium/dist/modules/libsodium.js",
        LIBSODIUM_JS,
    ),
    ("node_modules/libsodium/package.json", LIBSODIUM_PKG),
    (
        "node_modules/libsodium-wrappers/dist/modules/libsodium-wrappers.js",
        LIBSODIUM_WRAPPERS_JS,
    ),
    (
        "node_modules/libsodium-wrappers/package.json",
        LIBSODIUM_WRAPPERS_PKG,
    ),
];

/// Marker file written after a successful extraction so we only do it once.
const MARKER: &str = ".bypass_extracted";

// ── Public API ──────────────────────────────────────────────────────────────

/// Returns the absolute path to `bypass.js` inside the extracted cache dir.
/// Extracts embedded assets on first call (idempotent).
pub fn ensure_bypass_extracted() -> Result<PathBuf> {
    let cache_dir = resolve_cache_dir()?;
    let marker = cache_dir.join(MARKER);

    if !marker.exists() {
        extract_all(&cache_dir)?;
        // Write a small marker so subsequent runs skip extraction
        std::fs::write(&marker, env!("CARGO_PKG_VERSION"))
            .context("Writing bypass extraction marker")?;
        eprintln!(
            "[embegator] Extracted bypass runtime to {}",
            cache_dir.display()
        );
    }

    let bypass_js = cache_dir.join("bypass.js");
    if !bypass_js.exists() {
        // Marker exists but files were deleted — re-extract
        extract_all(&cache_dir)?;
    }

    Ok(bypass_js)
}

// ── Internals ───────────────────────────────────────────────────────────────

fn resolve_cache_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("Cannot determine executable path")?;
    let dir = exe
        .parent()
        .context("Executable has no parent directory")?
        .join("_bypass_cache");
    Ok(dir)
}

fn extract_all(root: &Path) -> Result<()> {
    for &(rel, content) in ASSETS {
        let dest = root.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating dir for {rel}"))?;
        }
        std::fs::write(&dest, content)
            .with_context(|| format!("Writing {rel}"))?;
    }
    Ok(())
}
