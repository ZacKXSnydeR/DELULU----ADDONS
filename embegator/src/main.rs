mod embedded_bypass;
mod models;
mod network;
mod proxy;

use clap::{Parser, Subcommand};
use models::{MediaQuery, MediaType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;

#[derive(Parser, Debug)]
#[command(name = "EmbeGator")]
#[command(author = "EmbeGator <https://github.com/ZacKXSnydeR>")]
#[command(version = "1.2.1")]
#[command(about = "External stream extractor addon runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short = 'j', long = "json", global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Movie {
        #[arg(short, long)]
        id: String,
    },
    Tv {
        #[arg(short, long)]
        id: String,
        #[arg(short, long)]
        season: u32,
        #[arg(short, long)]
        episode: u32,
    },
    Anime {
        #[arg(short, long)]
        id: String,
        #[arg(short, long)]
        season: u32,
        #[arg(short, long)]
        episode: u32,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRequest {
    id: Value,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveParams {
    media_type: String,
    tmdb_id: u32,
    season: Option<u32>,
    episode: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RpcResponse {
    id: Value,
    jsonrpc: String,
    protocol_version: String,
    result: Value,
}

fn to_media_query(params: ResolveParams) -> Result<MediaQuery, String> {
    let media_type = match params.media_type.to_lowercase().as_str() {
        "movie" => MediaType::Movie,
        "tv" => MediaType::TvShow,
        "anime" => MediaType::Anime,
        other => return Err(format!("Unsupported mediaType: {other}")),
    };
    if media_type != MediaType::Movie && (params.season.is_none() || params.episode.is_none()) {
        return Err("Season and episode are required for tv/anime".to_string());
    }
    Ok(MediaQuery {
        tmdb_id: params.tmdb_id.to_string(),
        media_type,
        season: params.season,
        episode: params.episode,
    })
}

async fn resolve_stream(params: ResolveParams) -> Value {
    let parsed = match to_media_query(params) {
        Ok(q) => q,
        Err(e) => return json!({ "success": false, "errorCode": "BAD_RESPONSE", "errorMessage": e }),
    };

    let output = match crate::network::fetch_media(parsed).await {
        Ok(o) => o,
        Err(e) => return json!({ "success": false, "errorCode": "UPSTREAM_ERROR", "errorMessage": e.to_string() }),
    };

    let first_stream = output.streams.first();
    let raw_url = match first_stream.and_then(|s| s.url.clone()) {
        Some(u) => u,
        None => return json!({ "success": false, "errorCode": "NO_STREAM", "errorMessage": "No playable stream returned by provider" }),
    };

    let subtitles: Vec<Value> = output
        .subtitles
        .iter()
        .filter_map(|s| {
            let url = s.url.clone()?;
            Some(json!({ "url": url, "language": s.language.clone().unwrap_or_else(|| "Unknown".to_string()) }))
        })
        .collect();

    let proxy_port = proxy::ensure_proxy_running().await;
    let proxy_base = format!("http://127.0.0.1:{}", proxy_port);
    let proxied_url = format!("{}/proxy?url={}", proxy_base, urlencoding::encode(&raw_url));

    json!({
        "success": true,
        "streamUrl": proxied_url,
        "headers": {},
        "subtitles": subtitles,
        "proxyPort": proxy_port,
        "selfProxy": true
    })
}

async fn run_rpc_mode() -> Result<(), Box<dyn Error>> {
    let stdin = std::io::stdin();
    let mut lines = std::io::BufRead::lines(stdin.lock());

    while let Some(Ok(line)) = lines.next() {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        let req: RpcRequest = match serde_json::from_str(&trimmed) {
            Ok(v) => v,
            Err(e) => {
                let out = json!({
                    "id": null, "jsonrpc": "2.0", "protocolVersion": "1.0",
                    "result": { "success": false, "errorCode": "BAD_RESPONSE", "errorMessage": format!("Invalid RPC request: {e}") }
                });
                println!("{}", serde_json::to_string(&out)?);
                continue;
            }
        };

        let result = match req.method.as_str() {
            "initialize" => json!({
                "ok": true,
                "name": "EmbeGator",
                "version": env!("CARGO_PKG_VERSION"),
                "protocolVersion": "1.0",
                "capabilities": ["stream.resolve", "subtitle.list", "health.check"]
            }),
            "healthCheck" => json!({
                "ok": true,
                "version": env!("CARGO_PKG_VERSION")
            }),
            "resolveStream" => {
                let parsed: Option<ResolveParams> =
                    req.params.clone().and_then(|v| serde_json::from_value(v).ok());
                match parsed {
                    Some(p) if p.tmdb_id > 0 => resolve_stream(p).await,
                    Some(_) => json!({ "success": false, "errorCode": "BAD_RESPONSE", "errorMessage": "tmdbId is required" }),
                    None => json!({ "success": false, "errorCode": "BAD_RESPONSE", "errorMessage": "Missing or invalid resolve params" }),
                }
            }
            _ => json!({ "success": false, "errorCode": "BAD_RESPONSE", "errorMessage": format!("Unknown method: {}", req.method) }),
        };

        let out = RpcResponse {
            id: req.id,
            jsonrpc: "2.0".to_string(),
            protocol_version: "1.0".to_string(),
            result,
        };
        println!("{}", serde_json::to_string(&out)?);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("rpc") {
        return run_rpc_mode().await;
    }

    let cli = Cli::parse();
    let query = match cli.command {
        Commands::Movie { id } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::Movie,
            season: None,
            episode: None,
        },
        Commands::Tv { id, season, episode } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::TvShow,
            season: Some(season),
            episode: Some(episode),
        },
        Commands::Anime { id, season, episode } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::Anime,
            season: Some(season),
            episode: Some(episode),
        },
    };

    match crate::network::fetch_media(query).await {
        Ok(result) => println!("{}", serde_json::to_string_pretty(&result)?),
        Err(e) => eprintln!("Error: {}", e),
    }
    Ok(())
}
