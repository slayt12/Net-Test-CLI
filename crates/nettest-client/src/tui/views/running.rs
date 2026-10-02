//! Running screen: header, stats panel, chart, log panel, key bar.

use nettest_proto::config::TestMode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use super::{chart, fmt_bytes, fmt_ms, fmt_secs, keys_line, level_style};
use crate::tui::app::{App, Link};

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let [header, body, logs, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(8),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, f, header);

    let [stats, graph] =
        Layout::horizontal([Constraint::Length(44), Constraint::Min(20)]).areas(body);
    render_stats(app, f, stats);
    if app.run_cfg.mode == TestMode::Throughput {
        chart::render_throughput(app, f, graph);
    } else {
        chart::render_rtt(app, f, graph);
    }
    render_log(app, f, logs);

    f.render_widget(
        Paragraph::new(keys_line(&[
            ("x", "stop"),
            ("r", "report now"),
            ("1/2/3", "chart window"),
            ("c", "clear log"),
            ("?", "help"),
            ("q", "quit"),
        ])),
        footer,
    );
}

fn render_header(app: &App, f: &mut Frame, area: Rect) {
    let (dot, label, color) = match app.link {
        Link::Connected => ("●", "connected".to_string(), Color::Green),
        Link::Reconnecting { attempt: 0 } => ("○", "disconnected".to_string(), Color::Yellow),
        Link::Reconnecting { attempt } => ("○", format!("reconnecting #{attempt}"), Color::Yellow),
        Link::Idle if app.is_running() => ("◌", "connecting".to_string(), Color::Gray),
        Link::Idle => ("◌", "finished".to_string(), Color::Gray),
    };
    let line = Line::from(vec![
        Span::styled(
            " nettest ",
            Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
        ),
        Span::raw(format!("  {}  ", app.run_cfg.target())),
        Span::styled(
            format!(
                "{} {}",
                app.run_cfg.mode,
                if app.stop_requested { "(stopping)" } else { "" }
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(format!("{dot} {label}"), Style::default().fg(color)),
        Span::raw("  "),
        Span::styled(
            format!("elapsed {}", fmt_secs(app.snapshot.elapsed_s)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_stats(app: &App, f: &mut Frame, area: Rect) {
    let l = &app.snapshot.latency;
    let mut rows: Vec<Row> = Vec::new();
    let kv = |k: &str, v: String, style: Style| {
        Row::new(vec![
            Cell::from(Span::styled(
                k.to_string(),
                Style::default().fg(Color::DarkGray),
            )),
            Cell::from(Span::styled(v, style)),
        ])
    };
    let plain = Style::default();
    let loss_style = if l.lost > 0 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };

    match app.run_cfg.mode {
        TestMode::Throughput => {
            if let Some(t) = &app.snapshot.throughput {
                rows.push(kv(
                    "instant",
                    format!("{:.1} Mbit/s", t.instant_mbps),
                    Style::default().fg(Color::Green).bold(),
                ));
                rows.push(kv("upload", format!("{:.1} Mbit/s", t.up_mbps), plain));
                rows.push(kv(
                    "  bytes / frames",
                    format!("{} / {}", fmt_bytes(t.up_bytes), t.up_frames),
                    plain,
                ));
                rows.push(kv("download", format!("{:.1} Mbit/s", t.down_mbps), plain));
                rows.push(kv(
                    "  bytes / frames",
                    format!("{} / {}", fmt_bytes(t.down_bytes), t.down_frames),
                    plain,
                ));
                if t.down_expected_frames > 0 {
                    rows.push(kv(
                        "  loss",
                        format!("{:.2}%", t.down_loss_pct),
                        if t.down_loss_pct > 0.0 {
                            Style::default().fg(Color::Red)
                        } else {
                            plain
                        },
                    ));
                }
            }
        }
        _ => {
            rows.push(kv(
                "sent / received",
                format!("{} / {}", l.sent, l.received),
                plain,
            ));
            rows.push(kv(
                "lost",
                format!("{} ({:.2}%)", l.lost, l.loss_pct),
                loss_style,
            ));
            rows.push(kv(
                "late / ooo / dup",
                format!("{} / {} / {}", l.late, l.out_of_order, l.duplicates),
                plain,
            ));
            rows.push(kv("in flight", l.in_flight.to_string(), plain));
            rows.push(kv(
                "last rtt",
                l.last_ms.map(fmt_ms).unwrap_or_else(|| "-".into()),
                Style::default().fg(Color::Cyan).bold(),
            ));
            rows.push(kv(
                "min / avg / max",
                format!("{:.2} / {:.2} / {:.2}", l.min_ms, l.avg_ms, l.max_ms),
                plain,
            ));
            rows.push(kv(
                "p50 / p95 / p99",
                format!("{:.2} / {:.2} / {:.2}", l.p50_ms, l.p95_ms, l.p99_ms),
                plain,
            ));
            rows.push(kv("jitter", fmt_ms(l.jitter_ms), plain));
            if let Some(s) = &app.snapshot.soak {
                rows.push(kv("", String::new(), plain));
                rows.push(kv(
                    "disconnects",
                    s.disconnects.to_string(),
                    if s.disconnects > 0 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        plain
                    },
                ));
                rows.push(kv("reconnects", s.reconnects.to_string(), plain));
                rows.push(kv("uptime", format!("{:.3}%", s.uptime_pct), plain));
                rows.push(kv(
                    "longest outage",
                    format!("{} ms", s.longest_outage_ms),
                    plain,
                ));
                rows.push(kv(
                    "session",
                    format!(
                        "{} (max {})",
                        fmt_secs(s.current_session_s),
                        fmt_secs(s.longest_session_s)
                    ),
                    plain,
                ));
            }
            if app.run_cfg.count > 0 || app.run_cfg.duration_secs > 0 {
                rows.push(kv("", String::new(), plain));
                if app.run_cfg.count > 0 {
                    rows.push(kv("target count", app.run_cfg.count.to_string(), plain));
                }
                if app.run_cfg.duration_secs > 0 {
                    rows.push(kv(
                        "target duration",
                        fmt_secs(app.run_cfg.duration_secs as f64),
                        plain,
                    ));
                }
            }
        }
    }

    let table = Table::new(rows, [Constraint::Length(17), Constraint::Min(10)])
        .block(Block::default().borders(Borders::ALL).title(" Stats "));
    f.render_widget(table, area);
}

fn render_log(app: &App, f: &mut Frame, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app
        .log
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
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Log ")),
        area,
    );
}
