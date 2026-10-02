//! Settings screen: a two-column field list with the selected field's hint and the key bar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::keys_line;
use crate::tui::app::App;
use crate::tui::form::{FIELDS, FieldKind, relevant, value_of};

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let [header, body, hint, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " nettest ",
                Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::raw("  client settings  "),
            Span::styled(
                format!("target {}", app.cfg.target()),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        header,
    );

    // Two columns when wide enough so all fields fit without scrolling on an 80x24 terminal.
    let two_cols = body.width >= 100;
    let per_col = if two_cols {
        FIELDS.len().div_ceil(2)
    } else {
        FIELDS.len()
    };
    let cols: Vec<Rect> = if two_cols {
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas::<2>(body)
            .to_vec()
    } else {
        vec![body]
    };

    for (ci, col) in cols.iter().enumerate() {
        let start = ci * per_col;
        let end = (start + per_col).min(FIELDS.len());
        let mut lines = Vec::with_capacity(per_col);
        let label_w = 26usize;
        for (i, def) in FIELDS[start..end].iter().enumerate() {
            let idx = start + i;
            let selected = idx == app.field;
            let active = relevant(&app.cfg, def.id);
            let value = if selected && app.editing.is_some() {
                format!("{}▏", app.editing.as_deref().unwrap_or(""))
            } else {
                value_of(&app.cfg, def.id)
            };
            let marker = match def.kind {
                FieldKind::Toggle => {
                    if value == "on" {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                }
                FieldKind::Cycle => "<> ",
                FieldKind::Text => "   ",
            };
            let value_style = match def.kind {
                _ if !active => Style::default().fg(Color::DarkGray),
                FieldKind::Toggle if value == "on" => Style::default().fg(Color::Green),
                FieldKind::Toggle => Style::default().fg(Color::DarkGray),
                _ => Style::default().fg(Color::White),
            };
            let value_shown = match def.kind {
                FieldKind::Toggle => String::new(),
                _ => value,
            };
            let row = Line::from(vec![
                Span::styled(
                    if selected { " ▶ " } else { "   " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{:<label_w$}", def.label),
                    if selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if !active {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(marker, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    value_shown,
                    if selected && app.editing.is_some() {
                        value_style.bg(Color::DarkGray)
                    } else {
                        value_style
                    },
                ),
            ]);
            lines.push(row);
        }
        let block = Block::default().borders(Borders::NONE);
        f.render_widget(Paragraph::new(lines).block(block), *col);
    }

    let def = &FIELDS[app.field];
    let hint_text = if app.editing.is_some() {
        "Enter commits, Esc cancels".to_string()
    } else if !relevant(&app.cfg, def.id) {
        format!(
            "{} (not used with {} / {})",
            def.hint, app.cfg.protocol, app.cfg.mode
        )
    } else {
        def.hint.to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("   ", Style::default()),
            Span::styled(hint_text, Style::default().fg(Color::DarkGray).italic()),
        ])),
        hint,
    );

    f.render_widget(
        Paragraph::new(keys_line(&[
            ("s", "start"),
            ("↑↓/Tab", "move"),
            ("Enter", "edit / toggle"),
            ("Space", "cycle"),
            ("w", "save"),
            ("?", "help"),
            ("q", "quit"),
        ])),
        footer,
    );
}
