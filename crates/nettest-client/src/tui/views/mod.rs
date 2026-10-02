//! Screen rendering. Pure functions of `App` state; no side effects.

mod chart;
mod form;
mod help;
mod running;
mod summary;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Screen};
use nettest_proto::sinks::LogLevel;

pub fn render(app: &mut App, f: &mut Frame) {
    let area = f.area();
    let [body, status] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).areas(area);

    match app.screen {
        Screen::Form => form::render(app, f, body),
        Screen::Running => running::render(app, f, body),
        Screen::Summary => summary::render(app, f, body),
    }
    render_status(app, f, status);
    if app.show_help {
        help::render(app, f, area);
    }
}

fn render_status(app: &App, f: &mut Frame, area: Rect) {
    let line = match &app.status {
        Some((level, text)) => Line::from(Span::styled(format!(" {text}"), level_style(*level))),
        None => Line::from(Span::styled(" ", Style::default())),
    };
    f.render_widget(Paragraph::new(line), area);
}

pub fn level_style(level: LogLevel) -> Style {
    match level {
        LogLevel::Debug => Style::default().fg(Color::DarkGray),
        LogLevel::Info => Style::default().fg(Color::Gray),
        LogLevel::Warn => Style::default().fg(Color::Yellow),
        LogLevel::Error => Style::default().fg(Color::Red).bold(),
    }
}

/// Footer key hints: "key" highlighted, description dim.
pub fn keys_line(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::with_capacity(pairs.len() * 3);
    for (k, d) in pairs {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ));
        spans.push(Span::styled(
            format!(" {d}  "),
            Style::default().fg(Color::Gray),
        ));
    }
    Line::from(spans)
}

pub fn fmt_ms(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} s", v / 1000.0)
    } else if v >= 100.0 {
        format!("{v:.0} ms")
    } else {
        format!("{v:.2} ms")
    }
}

pub fn fmt_secs(s: f64) -> String {
    let s = s as u64;
    if s >= 3600 {
        format!("{}h {:02}m {:02}s", s / 3600, (s % 3600) / 60, s % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn fmt_bytes(b: u64) -> String {
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
