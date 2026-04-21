use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState, Tabs, Wrap,
    },
    Frame,
};

use crate::app::{App, AppState, FocusedPane, InputMode};

// ── Palette ──────────────────────────────────────────────────────────────────

const CYAN:        Color = Color::Rgb(0,   255, 255);
const MAGENTA:     Color = Color::Rgb(255, 0,   255);
const NEON_GREEN:  Color = Color::Rgb(57,  255, 20);
const NEON_PINK:   Color = Color::Rgb(255, 20,  147);
const NEON_YELLOW: Color = Color::Rgb(255, 255, 0);
const DIM_BG:      Color = Color::Rgb(10,  10,  20);
const PANEL_BG:    Color = Color::Rgb(15,  15,  30);
const BORDER_DIM:  Color = Color::Rgb(40,  40,  80);
const TEXT_DIM:    Color = Color::Rgb(140, 140, 180);
const WHITE:       Color = Color::Rgb(230, 230, 255);

fn active_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BORDER_DIM)
    }
}

fn neon_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(if focused { CYAN } else { TEXT_DIM })
                .add_modifier(Modifier::BOLD),
        ))
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(active_border(focused))
        .style(Style::default().bg(PANEL_BG))
}

// ── Main draw entry-point ────────────────────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Background
    frame.render_widget(
        Block::default().style(Style::default().bg(DIM_BG)),
        area,
    );

    // Outer vertical split: header / body / footer
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(0),     // body
            Constraint::Length(3),  // footer / help bar
        ])
        .split(area);

    draw_header(frame, app, root[0]);
    draw_body(frame, app, root[1]);
    draw_footer(frame, root[2]);

    // Overlays
    if app.input_mode != InputMode::Normal {
        draw_input_popup(frame, app, area);
    }
    if let AppState::Error(ref msg) = app.state {
        draw_error_popup(frame, msg.clone(), area);
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(30)])
        .split(area);

    // ASCII brand
    let title_spans = Line::from(vec![
        Span::styled("◈ ", Style::default().fg(MAGENTA).bold()),
        Span::styled("NET", Style::default().fg(CYAN).bold()),
        Span::styled("DIAG", Style::default().fg(NEON_GREEN).bold()),
        Span::styled(" // Media Interoperability Tool", Style::default().fg(TEXT_DIM)),
    ]);
    frame.render_widget(
        Paragraph::new(title_spans)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(BORDER_DIM))
                    .style(Style::default().bg(PANEL_BG)),
            )
            .alignment(Alignment::Left),
        chunks[0],
    );

    // Status badge
    let (status_text, status_color) = match &app.state {
        AppState::Idle        => ("● IDLE",     TEXT_DIM),
        AppState::Fetching    => ("⟳ FETCHING", NEON_YELLOW),
        AppState::Done        => ("✔ DONE",     NEON_GREEN),
        AppState::Error(_)    => ("✖ ERROR",    NEON_PINK),
    };
    frame.render_widget(
        Paragraph::new(status_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(status_color))
                    .style(Style::default().bg(PANEL_BG)),
            )
            .style(Style::default().fg(status_color).bold())
            .alignment(Alignment::Center),
        chunks[1],
    );
}

// ── Body ─────────────────────────────────────────────────────────────────────

fn draw_body(frame: &mut Frame, app: &mut App, area: Rect) {
    // Left sidebar | Right main area
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(area);

    draw_sidebar(frame, app, cols[0]);
    draw_main_panel(frame, app, cols[1]);
}

// ── Sidebar (query + log) ─────────────────────────────────────────────────────

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(14), Constraint::Min(0)])
        .split(area);

    draw_query_panel(frame, app, rows[0]);
    draw_log_panel(frame, app, rows[1]);
}

fn draw_query_panel(frame: &mut Frame, app: &App, area: Rect) {
    use crate::app::MediaType;

    let focused = app.focused == FocusedPane::Query;
    let block   = neon_block("◈ TARGET SELECTION", focused);
    let inner   = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // spacer
            Constraint::Length(2), // TMDB ID
            Constraint::Length(2), // media type
            Constraint::Length(2), // season (conditional)
            Constraint::Length(2), // episode (conditional)
            Constraint::Min(0),
        ])
        .split(inner);

    // TMDB ID row
    let id_style = if focused { Style::default().fg(CYAN) } else { Style::default().fg(TEXT_DIM) };
    let id_val   = if app.tmdb_id.is_empty() {
        Span::styled("<press I to enter>", Style::default().fg(BORDER_DIM).italic())
    } else {
        Span::styled(app.tmdb_id.clone(), Style::default().fg(NEON_GREEN).bold())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("TMDB ID : ", id_style),
            id_val,
        ])),
        rows[1],
    );

    // Media type with tab-style selectors
    let type_spans: Vec<Span> = [MediaType::Movie, MediaType::TvShow, MediaType::Anime]
        .iter()
        .map(|t| {
            let label = format!(" {} ", t);
            if *t == app.media_type {
                Span::styled(label, Style::default().fg(DIM_BG).bg(CYAN).bold())
            } else {
                Span::styled(label, Style::default().fg(TEXT_DIM))
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Line::from(type_spans)),
        rows[2],
    );

    // Season
    if app.media_type != MediaType::Movie {
        let s_val = app.season.map(|s| s.to_string()).unwrap_or_else(|| "—".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Season  : ", Style::default().fg(TEXT_DIM)),
                Span::styled(s_val, Style::default().fg(NEON_YELLOW).bold()),
            ])),
            rows[3],
        );

        let e_val = app.episode.map(|e| e.to_string()).unwrap_or_else(|| "—".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Episode : ", Style::default().fg(TEXT_DIM)),
                Span::styled(e_val, Style::default().fg(NEON_YELLOW).bold()),
            ])),
            rows[4],
        );
    }
}

fn draw_log_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused == FocusedPane::Log;
    let block   = neon_block("◈ OPERATION LOG", focused);
    let inner   = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(inner.height as usize)
        .rev()
        .map(|(ts, msg)| {
            Line::from(vec![
                Span::styled(format!("{ts} "), Style::default().fg(BORDER_DIM)),
                Span::styled(msg.clone(), Style::default().fg(TEXT_DIM)),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true }),
        inner,
    );
}

// ── Main panel tabs ───────────────────────────────────────────────────────────

fn draw_main_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Tab bar
    let tabs = Tabs::new(vec!["  Streams  ", "  Subtitles  ", "  Raw JSON  "])
        .select(app.active_tab)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(BORDER_DIM))
                .style(Style::default().bg(PANEL_BG)),
        )
        .highlight_style(
            Style::default().fg(DIM_BG).bg(MAGENTA).bold(),
        )
        .divider(Span::styled("│", Style::default().fg(BORDER_DIM)));
    frame.render_widget(tabs, rows[0]);

    match app.active_tab {
        0 => draw_streams_tab(frame, app, rows[1]),
        1 => draw_subtitles_tab(frame, app, rows[1]),
        _ => draw_raw_tab(frame, app, rows[1]),
    }
}

// ── Streams tab ───────────────────────────────────────────────────────────────

fn draw_streams_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused == FocusedPane::Streams;
    let block   = neon_block("◈ STREAM MANIFESTS", focused);
    let inner   = block.inner(area);
    frame.render_widget(block, area);

    if app.result.streams.is_empty() {
        frame.render_widget(
            Paragraph::new(placeholder_text("No streams fetched yet.  Press F to fetch."))
                .alignment(Alignment::Center),
            centered_rect(inner, 60, 3),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(" Quality").style(Style::default().fg(NEON_GREEN).bold()),
        Cell::from(" Type   ").style(Style::default().fg(NEON_GREEN).bold()),
        Cell::from(" M3U8 URL").style(Style::default().fg(NEON_GREEN).bold()),
    ])
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .result
        .streams
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = app.stream_state.selected() == Some(i);
            let row_style = if selected {
                Style::default().fg(DIM_BG).bg(CYAN)
            } else {
                Style::default().fg(WHITE)
            };
            Row::new(vec![
                Cell::from(format!(" {}", s.quality.as_deref().unwrap_or("—"))),
                Cell::from(format!(" {}", s.kind.as_deref().unwrap_or("—"))),
                Cell::from(format!(" {}", s.url.as_deref().unwrap_or("—"))),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Length(10), Constraint::Length(24), Constraint::Min(0)],
    )
    .header(header)
    .block(Block::default())
    .highlight_style(Style::default().fg(DIM_BG).bg(CYAN))
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, inner, &mut app.stream_state);
}

// ── Subtitles tab ─────────────────────────────────────────────────────────────

fn draw_subtitles_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused == FocusedPane::Subtitles;
    let block   = neon_block("◈ SUBTITLE TRACKS", focused);
    let inner   = block.inner(area);
    frame.render_widget(block, area);

    if app.result.subtitles.is_empty() {
        frame.render_widget(
            Paragraph::new(placeholder_text("No subtitle tracks found."))
                .alignment(Alignment::Center),
            centered_rect(inner, 60, 3),
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(" Language").style(Style::default().fg(MAGENTA).bold()),
        Cell::from(" Kind    ").style(Style::default().fg(MAGENTA).bold()),
        Cell::from(" VTT URL ").style(Style::default().fg(MAGENTA).bold()),
    ])
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .result
        .subtitles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = app.subtitle_state.selected() == Some(i);
            let row_style = if selected {
                Style::default().fg(DIM_BG).bg(MAGENTA)
            } else {
                Style::default().fg(WHITE)
            };
            Row::new(vec![
                Cell::from(format!(" {}", s.display_lang())),
                Cell::from(format!(" {}", s.kind.as_deref().unwrap_or("—"))),
                Cell::from(format!(" {}", s.resolved_url().unwrap_or("—"))),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [Constraint::Length(16), Constraint::Length(14), Constraint::Min(0)],
    )
    .header(header)
    .block(Block::default())
    .highlight_style(Style::default().fg(DIM_BG).bg(MAGENTA))
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, inner, &mut app.subtitle_state);
}

// ── Raw JSON tab ──────────────────────────────────────────────────────────────

fn draw_raw_tab(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused == FocusedPane::Raw;
    let block   = neon_block("◈ RAW JSON PAYLOAD", focused);
    let inner   = block.inner(area);
    frame.render_widget(block, area);

    let text = if app.result.raw_json.is_empty() {
        Text::from(Span::styled(
            "No data yet.",
            Style::default().fg(BORDER_DIM).italic(),
        ))
    } else {
        // Syntax-tinted: keys in cyan, strings in green, numbers in yellow
        let colored: Vec<Line> = app
            .result
            .raw_json
            .lines()
            .skip(app.raw_scroll as usize)
            .take(inner.height as usize)
            .map(|line| colorize_json_line(line))
            .collect();
        Text::from(colored)
    };

    let max_scroll = app.result.raw_json.lines().count().saturating_sub(inner.height as usize);
    app.raw_scroll = app.raw_scroll.min(max_scroll as u16);

    frame.render_widget(Paragraph::new(text), inner);

    // Scrollbar
    if !app.result.raw_json.is_empty() {
        let mut sb_state = ScrollbarState::new(max_scroll).position(app.raw_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(BORDER_DIM)),
            area.inner(Margin { horizontal: 0, vertical: 1 }),
            &mut sb_state,
        );
    }
}

fn colorize_json_line(line: &str) -> Line<'static> {
    // Very lightweight tinting — no full parser needed
    let mut spans = Vec::new();
    let trimmed   = line.trim_start();
    let indent    = &line[..line.len() - trimmed.len()];
    spans.push(Span::raw(indent.to_string()));

    if trimmed.starts_with('"') && trimmed.contains("\":") {
        // Key-value pair
        let colon = trimmed.find("\":").unwrap_or(0);
        let key   = &trimmed[..=colon + 1];
        let rest  = &trimmed[colon + 2..];
        spans.push(Span::styled(key.to_string(),  Style::default().fg(CYAN)));
        spans.push(Span::styled(rest.to_string(), Style::default().fg(NEON_GREEN)));
    } else if trimmed.starts_with('"') {
        spans.push(Span::styled(trimmed.to_string(), Style::default().fg(NEON_GREEN)));
    } else if trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        spans.push(Span::styled(trimmed.to_string(), Style::default().fg(NEON_YELLOW)));
    } else {
        spans.push(Span::styled(trimmed.to_string(), Style::default().fg(TEXT_DIM)));
    }

    Line::from(spans)
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, area: Rect) {
    let keys: Vec<(&str, &str)> = vec![
        ("I",     "TMDB ID"),
        ("T",     "Media type"),
        ("S/E",   "Season/Ep"),
        ("F",     "Fetch"),
        ("Tab",   "Switch tab"),
        ("↑↓",    "Navigate"),
        ("PgU/D", "Scroll JSON"),
        ("Q",     "Quit"),
    ];
    let mut spans: Vec<Span> = Vec::new();
    for (key, desc) in &keys {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(DIM_BG).bg(CYAN).bold(),
        ));
        spans.push(Span::styled(
            format!(" {desc}  "),
            Style::default().fg(TEXT_DIM),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(BORDER_DIM))
                    .style(Style::default().bg(PANEL_BG)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

// ── Input popup ───────────────────────────────────────────────────────────────

fn draw_input_popup(frame: &mut Frame, app: &App, area: Rect) {
    let title = match app.input_mode {
        InputMode::TmdbId  => "Enter TMDB ID",
        InputMode::Season  => "Enter Season Number",
        InputMode::Episode => "Enter Episode Number",
        InputMode::Normal  => return,
    };

    let popup_area = centered_rect(area, 50, 7);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(Span::styled(
            format!(" ✎ {title} "),
            Style::default().fg(NEON_YELLOW).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(NEON_YELLOW))
        .style(Style::default().bg(PANEL_BG));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Input line with blinking cursor
    let cursor_char = if (chrono::Local::now().timestamp_millis() / 500) % 2 == 0 { "█" } else { " " };
    let display = format!("{}{}", app.input_buf, cursor_char);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                display,
                Style::default().fg(NEON_GREEN).bold(),
            )),
            Line::from(Span::styled(
                " Enter to confirm  Esc to cancel",
                Style::default().fg(TEXT_DIM).italic(),
            )),
        ])
        .alignment(Alignment::Center),
        inner,
    );
}

// ── Error popup ───────────────────────────────────────────────────────────────

fn draw_error_popup(frame: &mut Frame, msg: String, area: Rect) {
    let popup_area = centered_rect(area, 60, 9);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(Span::styled(
            " ✖ ERROR ",
            Style::default().fg(NEON_PINK).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(NEON_PINK))
        .style(Style::default().bg(PANEL_BG));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                msg,
                Style::default().fg(NEON_PINK),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press any key to dismiss",
                Style::default().fg(TEXT_DIM).italic(),
            )),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        inner,
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn placeholder_text(msg: &str) -> Text<'static> {
    Text::from(Line::from(Span::styled(
        msg.to_string(),
        Style::default().fg(BORDER_DIM).italic(),
    )))
}

/// Returns a centred Rect of given % width and fixed height.
fn centered_rect(area: Rect, percent_w: u16, height: u16) -> Rect {
    let pw = area.width * percent_w / 100;
    let x  = area.x + (area.width.saturating_sub(pw)) / 2;
    let y  = area.y + (area.height.saturating_sub(height)) / 2;
    Rect { x, y, width: pw.min(area.width), height: height.min(area.height) }
}
