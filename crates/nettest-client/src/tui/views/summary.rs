//! Post-run summary: final numbers, connect phases, where files went.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::{chart, fmt_bytes, fmt_secs, keys_line};
use crate::tui::app::App;
use nettest_proto::config::TestMode;

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " nettest ",
                Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::raw("  run finished  "),
            Span::styled(app.run_cfg.target(), Style::default().fg(Color::DarkGray)),
        ])),
        header,
    );

    let [text_area, graph] =
        Layout::horizontal([Constraint::Length(56), Constraint::Min(20)]).areas(body);

    let mut lines: Vec<Line> = Vec::new();
    let dim = Style::default().fg(Color::DarkGray);
    let kv =
        |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<18}"), dim), Span::raw(v)]);
    if let Some(out) = &app.last_output {
        let s = &out.summary;
        let l = &s.latency;
        lines.push(kv("result", s.stop_reason.clone()));
        lines.push(kv(
            "mode",
            format!(
                "{} over {}",
                s.mode,
                s.protocol.map(|p| p.to_string()).unwrap_or_default()
            ),
        ));
        if l.sent > 0 {
            lines.push(Line::from(""));
            lines.push(kv(
                "sent / received",
                format!("{} / {}", l.sent, l.received),
            ));
            lines.push(Line::from(vec![
                Span::styled(format!("{:<18}", "lost"), dim),
                Span::styled(
                    format!("{} ({:.2}%)", l.lost, l.loss_pct),
                    if l.lost > 0 {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Green)
                    },
                ),
            ]));
            lines.push(kv(
                "late / ooo / dup",
                format!("{} / {} / {}", l.late, l.out_of_order, l.duplicates),
            ));
            lines.push(kv(
                "rtt min/avg/max",
                format!("{:.2} / {:.2} / {:.2} ms", l.min_ms, l.avg_ms, l.max_ms),
            ));
            lines.push(kv(
                "rtt p50/p95/p99",
                format!("{:.2} / {:.2} / {:.2} ms", l.p50_ms, l.p95_ms, l.p99_ms),
            ));
            lines.push(kv("jitter", format!("{:.2} ms", l.jitter_ms)));
            lines.push(kv("duration", fmt_secs(l.elapsed.as_secs_f64())));
        }
        if let Some(t) = &s.throughput {
            lines.push(Line::from(""));
            if t.up_bytes > 0 {
                lines.push(kv(
                    "upload",
                    format!(
                        "{:.2} Mbit/s ({} in {:.1}s)",
                        t.up_mbps,
                        fmt_bytes(t.up_bytes),
                        t.up_secs
                    ),
                ));
            }
            if t.down_bytes > 0 {
                lines.push(kv(
                    "download",
                    format!(
                        "{:.2} Mbit/s ({} in {:.1}s{})",
                        t.down_mbps,
                        fmt_bytes(t.down_bytes),
                        t.down_secs,
                        if t.down_expected_frames > 0 {
                            format!(", {:.2}% lost", t.down_loss_pct)
                        } else {
                            String::new()
                        }
                    ),
                ));
            }
        }
        if let Some(k) = &s.soak {
            lines.push(Line::from(""));
            lines.push(kv(
                "disconnects",
                format!("{} ({} reconnects)", k.disconnects, k.reconnects),
            ));
            lines.push(kv("uptime", format!("{:.3}%", k.uptime_pct)));
            lines.push(kv("longest outage", format!("{} ms", k.longest_outage_ms)));
        }
        if let Some(c) = s.connects.first() {
            let ms = |d: Option<std::time::Duration>| {
                d.map(|d| format!("{:.1}", d.as_secs_f64() * 1e3))
                    .unwrap_or_else(|| "-".into())
            };
            lines.push(Line::from(""));
            lines.push(kv(
                "connect ms",
                format!(
                    "dns {} tcp {} tls {} ws {} hello {}",
                    ms(c.dns),
                    ms(c.tcp),
                    ms(c.tls),
                    ms(c.ws_upgrade),
                    ms(c.hello)
                ),
            ));
            lines.push(kv("connects", s.connects.len().to_string()));
        }
    } else {
        lines.push(Line::from("collecting final results..."));
    }
    if let Some(p) = &app.report_path {
        lines.push(Line::from(""));
        lines.push(kv("report", p.display().to_string()));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Summary ")),
        text_area,
    );

    if app.run_cfg.mode == TestMode::Throughput {
        chart::render_throughput(app, f, graph);
    } else {
        chart::render_rtt(app, f, graph);
    }

    f.render_widget(
        Paragraph::new(keys_line(&[
            ("s", "run again"),
            ("Enter", "back to settings"),
            ("r", "write report"),
            ("1/2/3", "chart window"),
            ("q", "quit"),
        ])),
        footer,
    );
}
