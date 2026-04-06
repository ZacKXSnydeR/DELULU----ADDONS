mod models;
mod network;

use clap::{Parser, Subcommand};
use models::{MediaQuery, MediaType};
use std::error::Error;

#[derive(Parser, Debug)]
#[command(name = "EmbeGator")]
#[command(author = "EmbeGator <https://github.com/ZacKXSnydeR>")]
#[command(version = "1.0")]
#[command(about = "External stream extractor addon runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output raw JSON instead of styled text
    #[arg(short = 'j', long = "json", global = true)]
    json: bool,

    /// Absolute path to the bypass.js script
    #[arg(long = "bypass-path", global = true)]
    bypass_path: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Extract a movie stream and subtitles
    Movie {
        /// The TMDB ID of the movie
        #[arg(short, long)]
        id: String,
    },
    /// Extract a TV show stream and subtitles
    Tv {
        /// The TMDB ID of the TV show
        #[arg(short, long)]
        id: String,

        /// Season number
        #[arg(short, long)]
        season: u32,

        /// Episode number
        #[arg(short, long)]
        episode: u32,
    },
    /// Extract an Anime stream and subtitles
    Anime {
        /// The TMDB ID of the Anime
        #[arg(short, long)]
        id: String,

        /// Season number
        #[arg(short, long)]
        season: u32,

        /// Episode number
        #[arg(short, long)]
        episode: u32,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if !cli.json {
        // Print the ASCII Banner
        println!("\x1b[1;36m");
        println!("  _____           _         ______");
        println!(" / ____|         | |       |  ____|");
        println!("| |  __  ___   __| |___    | |__  _   _  ___ ");
        println!("| | |_ |/ _ \\ / _` / __|   |  __|| | | |/ _ \\");
        println!("| |__| | (_) | (_| \\__ \\   | |___| |_| |  __/");
        println!(" \\_____|\\___/ \\__,_|___/   |______\\__, |\\___|");
        println!("                                   __/ |");
        println!("                                  |___/ \x1b[0m\n");
        println!("\x1b[1;35m  :: Vidlink Direct Extractor ::\x1b[0m\n");
    }

    let query = match cli.command {
        Commands::Movie { id } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::Movie,
            season: None,
            episode: None,
            bypass_path: cli.bypass_path.clone(),
        },
        Commands::Tv {
            id,
            season,
            episode,
        } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::TvShow,
            season: Some(season),
            episode: Some(episode),
            bypass_path: cli.bypass_path.clone(),
        },
        Commands::Anime {
            id,
            season,
            episode,
        } => MediaQuery {
            tmdb_id: id,
            media_type: MediaType::Anime,
            season: Some(season),
            episode: Some(episode),
            bypass_path: cli.bypass_path.clone(),
        },
    };

    if !cli.json {
        println!(
            "\x1b[1;33m[*] Fetching {} stream for ID: {}...\x1b[0m",
            query.media_type, query.tmdb_id
        );
    }
    match crate::network::fetch_media(query).await {
        Ok(result) => {
            if cli.json {
                if let Ok(pretty) = serde_json::to_string_pretty(&result) {
                    println!("{}", pretty);
                } else {
                    eprintln!(r#"{{"error": "Failed to serialize JSON"}}"#);
                }
            } else {
                println!("\x1b[1;32m[+] Success!\x1b[0m");

                println!("\n\x1b[1;36m=== 🎥 STREAM LINKS ===\x1b[0m");
                if result.streams.is_empty() {
                    println!("  No stream links found.");
                } else {
                    for (i, stream) in result.streams.iter().enumerate() {
                        let qual = stream.quality.as_deref().unwrap_or("Unknown");
                        println!("\x1b[1;37m[{}] Quality: {}\x1b[0m", i + 1, qual);
                        println!(
                            "    Link: \x1b[4m{}\x1b[0m",
                            stream.url.as_deref().unwrap_or("N/A")
                        );
                    }
                }

                println!("\n\x1b[1;36m=== 📝 SUBTITLE LINKS ===\x1b[0m");
                if result.subtitles.is_empty() {
                    println!("  No subtitles found.");
                } else {
                    for (i, sub) in result.subtitles.iter().enumerate() {
                        let lang = sub.language.as_deref().unwrap_or("Unknown");
                        println!("\x1b[1;37m[{}] Language: {}\x1b[0m", i + 1, lang);
                        println!(
                            "    Link: \x1b[4m{}\x1b[0m",
                            sub.url.as_deref().unwrap_or("N/A")
                        );
                    }
                }

                println!("\n\x1b[1;36m=== 🔒 HEADERS ===\x1b[0m");
                if let Some(stream) = result.streams.first() {
                    if let Some(headers) = &stream.headers {
                        println!(
                            "  Referer: {}",
                            headers.referer.as_deref().unwrap_or("https://vidlink.pro/")
                        );
                        println!(
                            "  Origin: {}",
                            headers.origin.as_deref().unwrap_or("https://vidlink.pro")
                        );
                    } else {
                        println!("  Referer: https://vidlink.pro/");
                        println!("  Origin: https://vidlink.pro");
                    }
                } else {
                    println!("  Referer: https://vidlink.pro/");
                    println!("  Origin: https://vidlink.pro");
                }
                println!("  User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36\n");
            }
        }
        Err(e) => {
            if cli.json {
                eprintln!(r#"{{"error": "{}"}}"#, e);
            } else {
                eprintln!("\n\x1b[1;31m[-] Error Extracing: {}\x1b[0m", e);
            }
        }
    }

    Ok(())
}
