use ratatui::widgets::TableState;
use tokio::sync::mpsc;

use crate::models::{MediaResult, MediaQuery};

// ── Re-exports so ui.rs only needs `crate::app::*` ──────────────────────────

pub use crate::models::MediaType;

// ── State machine ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    Fetching,
    Done,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    TmdbId,
    Season,
    Episode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusedPane {
    Query,
    Log,
    Streams,
    Subtitles,
    Raw,
}

// ── Messages from background tasks ───────────────────────────────────────────

pub enum NetEvent {
    Log(String),
    Done(MediaResult),
    Error(String),
}

// ── Application ───────────────────────────────────────────────────────────────

pub struct App {
    pub state:          AppState,
    pub input_mode:     InputMode,
    pub focused:        FocusedPane,
    pub active_tab:     usize,      // 0=Streams 1=Subtitles 2=Raw

    // Query fields
    pub tmdb_id:        String,
    pub media_type:     MediaType,
    pub season:         Option<u32>,
    pub episode:        Option<u32>,

    // Input buffer (shared across popup modes)
    pub input_buf:      String,

    // Results
    pub result:         MediaResult,
    pub stream_state:   TableState,
    pub subtitle_state: TableState,
    pub raw_scroll:     u16,

    // Log
    pub log: Vec<(String, String)>,   // (timestamp, message)

    // Channel to receive events from network task
    pub net_rx: mpsc::Receiver<NetEvent>,
    pub net_tx: mpsc::Sender<NetEvent>,
}

impl App {
    pub fn new() -> Self {
        let (net_tx, net_rx) = mpsc::channel(64);
        let mut s = Self {
            state:          AppState::Idle,
            input_mode:     InputMode::Normal,
            focused:        FocusedPane::Query,
            active_tab:     0,
            tmdb_id:        String::new(),
            media_type:     MediaType::Movie,
            season:         None,
            episode:        None,
            input_buf:      String::new(),
            result:         MediaResult::default(),
            stream_state:   TableState::default(),
            subtitle_state: TableState::default(),
            raw_scroll:     0,
            log:            Vec::new(),
            net_rx,
            net_tx,
        };
        s.push_log("Ready. Press I to enter a TMDB ID, then F to fetch.");
        s
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        let ts = chrono::Local::now().format("%H:%M:%S").to_string();
        self.log.push((ts, msg.into()));
        // Keep last 200 entries
        if self.log.len() > 200 {
            self.log.drain(0..100);
        }
    }

    /// Drain all pending network events without blocking.
    pub fn poll_net_events(&mut self) {
        while let Ok(ev) = self.net_rx.try_recv() {
            match ev {
                NetEvent::Log(msg)  => self.push_log(msg),
                NetEvent::Done(res) => {
                    self.push_log(format!(
                        "✔ Done — {} stream(s), {} subtitle(s)",
                        res.streams.len(),
                        res.subtitles.len()
                    ));
                    self.result = res;
                    self.state  = AppState::Done;
                    self.active_tab = 0;
                    if !self.result.streams.is_empty() {
                        self.stream_state.select(Some(0));
                    }
                    if !self.result.subtitles.is_empty() {
                        self.subtitle_state.select(Some(0));
                    }
                }
                NetEvent::Error(e) => {
                    self.push_log(format!("✖ Error: {e}"));
                    self.state = AppState::Error(e);
                }
            }
        }
    }

    /// Kick off an async fetch in a background task.
    pub fn start_fetch(&mut self) {
        if self.tmdb_id.is_empty() {
            self.push_log("✖ No TMDB ID entered.");
            return;
        }
        if self.media_type != MediaType::Movie
            && (self.season.is_none() || self.episode.is_none())
        {
            self.push_log("✖ Season and Episode required for TV/Anime.");
            return;
        }

        self.state  = AppState::Fetching;
        self.result = MediaResult::default();
        self.raw_scroll = 0;

        let query = MediaQuery {
            tmdb_id:    self.tmdb_id.clone(),
            media_type: self.media_type.clone(),
            season:     self.season,
            episode:    self.episode,
        };
        let tx = self.net_tx.clone();
        self.push_log(format!("⟳ Fetching {} id={} …", query.media_type, query.tmdb_id));

        tokio::spawn(async move {
            let _ = tx.send(NetEvent::Log("Generating token …".into())).await;
            match crate::network::fetch_media(query).await {
                Ok(res)  => { let _ = tx.send(NetEvent::Done(res)).await; }
                Err(err) => { let _ = tx.send(NetEvent::Error(err.to_string())).await; }
            }
        });
    }

    // ── Navigation helpers ─────────────────────────────────────────────────

    pub fn next_tab(&mut self)    { self.active_tab = (self.active_tab + 1) % 3; }
    pub fn prev_tab(&mut self)    { self.active_tab = self.active_tab.saturating_sub(1); }
    pub fn cycle_media_type(&mut self) {
        self.media_type = match self.media_type {
            MediaType::Movie  => MediaType::TvShow,
            MediaType::TvShow => MediaType::Anime,
            MediaType::Anime  => MediaType::Movie,
        };
    }

    pub fn table_down(&mut self) {
        match self.active_tab {
            0 => {
                let len = self.result.streams.len();
                if len > 0 {
                    let i = self.stream_state.selected().unwrap_or(0);
                    self.stream_state.select(Some((i + 1) % len));
                }
            }
            1 => {
                let len = self.result.subtitles.len();
                if len > 0 {
                    let i = self.subtitle_state.selected().unwrap_or(0);
                    self.subtitle_state.select(Some((i + 1) % len));
                }
            }
            _ => { self.raw_scroll = self.raw_scroll.saturating_add(3); }
        }
    }

    pub fn table_up(&mut self) {
        match self.active_tab {
            0 => {
                let len = self.result.streams.len();
                if len > 0 {
                    let i = self.stream_state.selected().unwrap_or(0);
                    self.stream_state.select(Some(if i == 0 { len - 1 } else { i - 1 }));
                }
            }
            1 => {
                let len = self.result.subtitles.len();
                if len > 0 {
                    let i = self.subtitle_state.selected().unwrap_or(0);
                    self.subtitle_state.select(Some(if i == 0 { len - 1 } else { i - 1 }));
                }
            }
            _ => { self.raw_scroll = self.raw_scroll.saturating_sub(3); }
        }
    }

    pub fn page_down(&mut self) { self.raw_scroll = self.raw_scroll.saturating_add(20); }
    pub fn page_up(&mut self)   { self.raw_scroll = self.raw_scroll.saturating_sub(20); }
}
