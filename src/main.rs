mod models;
mod network;

use clap::{Parser, Subcommand};
use models::{MediaQuery, MediaType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser, Debug)]
#[command(name = "EmbeGator")]
#[command(author = "EmbeGator <https://github.com/ZacKXSnydeR>")]
#[command(version = "1.0")]
#[command(about = "External stream extractor addon runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short = 'j', long = "json", global = true)]
    json: bool,

    #[arg(long = "bypass-path", global = true)]
    bypass_path: Option<String>,
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

fn to_media_query(params: ResolveParams, bypass_path: Option<String>) -> Result<MediaQuery, String> {
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
        bypass_path,
    })
}

async fn run_rpc_mode() -> Result<(), Box<dyn Error>> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let maybe_line = lines.next_line().await?;
    let Some(line) = maybe_line else {
        return Ok(());
    };
    let req: RpcRequest = match serde_json::from_str(&line) {
        Ok(v) => v,
        Err(e) => {
            let out = json!({
                "id": null,
                "jsonrpc": "2.0",
                "protocolVersion": "1.0",
                "result": {
                    "success": false,
                    "errorCode": "BAD_RESPONSE",
                    "errorMessage": format!("Invalid RPC request: {e}")
                }
            });
            println!("{}", serde_json::to_string(&out)?);
            return Ok(());
        }
    };

    let bypass_path = std::env::var("EMBEGATOR_BYPASS_PATH").ok();
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
            let parsed_opt: Option<ResolveParams> =
                req.params.clone().and_then(|v| serde_json::from_value(v).ok());
            if parsed_opt.is_none() {
                json!({
                    "success": false,
                    "errorCode": "BAD_RESPONSE",
                    "errorMessage": "Missing or invalid resolve params"
                })
            } else {
                let parsed = parsed_opt.expect("checked above");
                if parsed.tmdb_id == 0 {
                    json!({
                        "success": false,
                        "errorCode": "BAD_RESPONSE",
                        "errorMessage": "tmdbId is required"
                    })
                } else {
                    match to_media_query(parsed, bypass_path) {
                        Ok(query) => match crate::network::fetch_media(query).await {
                            Ok(output) => {
                                let first_stream = output.streams.first();
                                let stream_url = first_stream.and_then(|s| s.url.clone());
                                let headers = first_stream
                                    .and_then(|s| s.headers.clone())
                                    .map(|h| {
                                        json!({
                                            "Referer": h.referer,
                                            "Origin": h.origin
                                        })
                                    })
                                    .unwrap_or_else(|| {
                                        json!({
                                            "Referer": "https://vidlink.pro/",
                                            "Origin": "https://vidlink.pro"
                                        })
                                    });

                                let subtitles = output
                                    .subtitles
                                    .iter()
                                    .filter_map(|s| {
                                        let url = s.url.clone()?;
                                        Some(json!({
                                            "url": url,
                                            "language": s.language.clone().unwrap_or_else(|| "Unknown".to_string())
                                        }))
                                    })
                                    .collect::<Vec<_>>();

                                if stream_url.is_some() {
                                    json!({
                                        "success": true,
                                        "streamUrl": stream_url,
                                        "headers": headers,
                                        "subtitles": subtitles
                                    })
                                } else {
                                    json!({
                                        "success": false,
                                        "errorCode": "NO_STREAM",
                                        "errorMessage": "No playable stream returned by provider"
                                    })
                                }
                            }
                            Err(e) => json!({
                                "success": false,
                                "errorCode": "UPSTREAM_ERROR",
                                "errorMessage": e.to_string()
                            }),
                        },
                        Err(err) => json!({
                            "success": false,
                            "errorCode": "BAD_RESPONSE",
                            "errorMessage": err
                        }),
                    }
                }
            }
        }
        _ => json!({
            "success": false,
            "errorCode": "BAD_RESPONSE",
            "errorMessage": format!("Unknown method: {}", req.method)
        }),
    };

    let out = RpcResponse {
        id: req.id,
        jsonrpc: "2.0".to_string(),
        protocol_version: "1.0".to_string(),
        result,
    };
    println!("{}", serde_json::to_string(&out)?);
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
            bypass_path: cli.bypass_path.clone(),
        },
        Commands::Tv { id, season, episode } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::TvShow,
            season: Some(season),
            episode: Some(episode),
            bypass_path: cli.bypass_path.clone(),
        },
        Commands::Anime { id, season, episode } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::Anime,
            season: Some(season),
            episode: Some(episode),
            bypass_path: cli.bypass_path.clone(),
        },
    };

    match crate::network::fetch_media(query).await {
        Ok(result) => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        Err(e) => {
            if cli.json {
                eprintln!(r#"{{"error":"{}"}}"#, e);
            } else {
                eprintln!("Error: {}", e);
            }
        }
    }
    Ok(())
}
