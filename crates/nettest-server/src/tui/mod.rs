//! Server terminal UI: listeners and identity on top, live client table in the middle, log tail
//! at the bottom. Settings are taken from flags/config at start-up (rebinding sockets live is
//! not worth the complexity for a diagnostics server); `w` saves the effective settings.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use nettest_proto::sinks::LogLevel;
use nettest_server::RunningServer;
use nettest_server::log::LogTail;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

struct Ui<'a> {
    server: &'a RunningServer,
    tail: LogTail,
    config_path: PathBuf,
    status: Option<(LogLevel, String)>,
    show_help: bool,
}

pub async fn run(
    server: &RunningServer,
    tail: LogTail,
    config_path: PathBuf,
) -> anyhow::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));
    let mut terminal = ratatui::init();
    let mut ui = Ui {
        server,
        tail,
        config_path,
        status: None,
        show_help: false,
    };
    let result = event_loop(&mut terminal, &mut ui).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    ui: &mut Ui<'_>,
) -> anyhow::Result<()> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        terminal.draw(|f| render(ui, f))?;
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => {
                    if ui.show_help {
                        ui.show_help = false;
                        continue;
                    }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Char('c') => {
                            if let Ok(mut t) = ui.tail.lock() {
                                t.clear();
                            }
                        }
                        KeyCode::Char('w') => {
                            ui.status = Some(match nettest_proto::config::save(&ui.config_path, &ui.server.ctx.config) {
                                Ok(()) => (LogLevel::Info, format!("settings saved to {}", ui.config_path.display())),
                                Err(e) => (LogLevel::Error, format!("save failed: {e}")),
                            });
                        }
                        KeyCode::Char('?') => ui.show_help = true,
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => anyhow::bail!("terminal input error: {e}"),
                None => break,
            },
            _ = ui.server.ctx.cancel.cancelled() => break,
            _ = tick.tick() => {}
        }
    }
    Ok(())
}

fn render(ui: &Ui<'_>, f: &mut Frame) {
    let area = f.area();
    let [header, info, clients, logs, footer, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Min(6),
        Constraint::Length(10),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let sessions = ui.server.ctx.sessions.snapshot();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " nettest ",
                Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::raw(format!("  server v{}  ", nettest_server::version())),
            Span::styled(
                format!(
                    "{} client(s) connected, {} total",
                    sessions.len(),
                    ui.server.ctx.sessions.total_accepted()
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        header,
    );

    render_info(ui, f, info);
    render_clients(&sessions, f, clients);
    render_log(ui, f, logs);

    let keys = [
        ("c", "clear log"),
        ("w", "save settings"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = Vec::new();
    for (k, d) in keys {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ));
        spans.push(Span::styled(
            format!(" {d}  "),
            Style::default().fg(Color::Gray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), footer);

    let st = match &ui.status {
        Some((lvl, t)) => Line::from(Span::styled(format!(" {t}"), level_style(*lvl))),
        None => Line::from(""),
    };
    f.render_widget(Paragraph::new(st), status);

    if ui.show_help {
        render_help(f, area);
    }
}

fn render_info(ui: &Ui<'_>, f: &mut Frame, area: Rect) {
    let listeners: Vec<String> = ui
        .server
        .listeners
        .iter()
        .map(|(p, a)| format!("{p}://{a}"))
        .collect();
    let cfg = &ui.server.ctx.config;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("listening  ", Style::default().fg(Color::DarkGray)),
            Span::raw(listeners.join("   ")),
        ]),
        Line::from(vec![
            Span::styled("auth       ", Style::default().fg(Color::DarkGray)),
            if cfg.token.is_empty() {
                Span::styled("open (no token)", Style::default().fg(Color::Yellow))
            } else {
                Span::styled("token required", Style::default().fg(Color::Green))
            },
            Span::styled(
                format!(
                    "     idle timeout {}s     logs: {}{}",
                    cfg.idle_timeout_secs,
                    if cfg.log_file.is_empty() {
                        "-"
                    } else {
                        cfg.log_file.as_str()
                    },
                    if cfg.jsonl_file.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", cfg.jsonl_file)
                    }
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];
    if let Some(fp) = &ui.server.fingerprint {
        lines.push(Line::from(vec![
            Span::styled("wss sha256 ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                nettest_proto::tls::fingerprint::to_hex(fp),
                Style::default().fg(Color::Cyan),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Server ")),
        area,
    );
}

fn render_clients(sessions: &[nettest_server::session::SessionStats], f: &mut Frame, area: Rect) {
    let header = Row::new(
        [
            "id", "proto", "peer", "client", "mode", "age", "idle", "probes", "in", "out",
        ]
        .into_iter()
        .map(|h| Cell::from(Span::styled(h, Style::default().fg(Color::DarkGray)))),
    );
    let rows: Vec<Row> = sessions
        .iter()
        .map(|s| {
            let idle = s.last_seen.elapsed().as_secs_f64();
            let idle_style = if idle > 10.0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(s.id.to_string()),
                Cell::from(s.protocol.to_string()),
                Cell::from(s.peer.to_string()),
                Cell::from(if s.authed {
                    s.client_id.clone()
                } else {
                    "(unauthenticated)".into()
                }),
                Cell::from(s.mode.clone()),
                Cell::from(fmt_secs(s.connected_at.elapsed().as_secs_f64())),
                Cell::from(Span::styled(format!("{idle:.0}s"), idle_style)),
                Cell::from(s.probes.to_string()),
                Cell::from(fmt_bytes(s.bytes_in)),
                Cell::from(fmt_bytes(s.bytes_out)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(24),
            Constraint::Min(14),
            Constraint::Length(11),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Clients ({}) ", sessions.len())),
    );
    f.render_widget(table, area);
}

fn render_log(ui: &Ui<'_>, f: &mut Frame, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = match ui.tail.lock() {
        Ok(t) => t
            .iter()
            .rev()
            .take(inner_h)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|l| {
                Line::from(vec![
                    Span::styled(
                        l.at.format("%H:%M:%S%.3f ").to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(l.text.clone(), level_style(l.level)),
                ])
            })
            .collect(),
        Err(_) => vec![],
    };
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Log ")),
        area,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    use ratatui::layout::Flex;
    use ratatui::widgets::Clear;
    let text = [
        "Settings come from flags, environment (NETTEST_*) and the config file.",
        "Run with --help for the full list, or --no-tui to log to stderr instead.",
        "",
        "  c   clear the log panel",
        "  w   save the effective settings to the config file",
        "  q   stop the server and quit",
        "",
        "Clients connect with:  nettest-client ws://<this host>:<ws port>",
        "For wss, hand them the SHA-256 fingerprint shown above.",
    ];
    let w = 76.min(area.width.saturating_sub(2));
    let h = (text.len() as u16 + 2).min(area.height.saturating_sub(2));
    let [v] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(text.iter().map(|s| Line::from(*s)).collect::<Vec<_>>()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help (any key to close) ")
                .style(Style::default().fg(Color::White).bg(Color::Black)),
        ),
        popup,
    );
}

fn level_style(level: LogLevel) -> Style {
    match level {
        LogLevel::Debug => Style::default().fg(Color::DarkGray),
        LogLevel::Info => Style::default().fg(Color::Gray),
        LogLevel::Warn => Style::default().fg(Color::Yellow),
        LogLevel::Error => Style::default().fg(Color::Red),
    }
}

fn fmt_secs(s: f64) -> String {
    let s = s as u64;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.2} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}
