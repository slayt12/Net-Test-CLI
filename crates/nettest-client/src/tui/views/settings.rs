//! Settings screen: tab bar, the current tab's rows, an optional result pane, the selected
//! row's hint and the key bar. One renderer for all four tabs; rows come from `App::rows`.

use nettest_proto::sinks::LogLevel;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use super::{keys_line, level_style};
use crate::tui::app::{App, Tab};
use crate::tui::edit::Editor;
use crate::tui::form::FieldKind;
use crate::tui::rows::Row;

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let rows = app.rows();
    let results: &[(LogLevel, String)] = match app.tab {
        Tab::Test => &[],
        Tab::Alerts => &app.alerts_results,
        Tab::Monitor => &app.builder_results,
        Tab::Service => &app.service.output,
    };
    let results_h = if results.is_empty() {
        0
    } else {
        (results.len() as u16 + 2).min(area.height / 3).max(4)
    };
    let [header, body, pane, hint, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(results_h),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, f, header);

    // The Test tab keeps its two-column layout so every field fits on 80x24 without scrolling;
    // the dynamic tabs scroll a single column.
    let two_cols = app.tab == Tab::Test && body.width >= 100;
    render_rows(f, body, &rows, app.cursor(), app.editing.as_ref(), two_cols);

    if results_h > 0 {
        let visible = results_h.saturating_sub(2) as usize;
        let start = results.len().saturating_sub(visible);
        let lines: Vec<Line> = results[start..]
            .iter()
            .map(|(lvl, t)| Line::from(Span::styled(t.clone(), level_style(*lvl))))
            .collect();
        let title = match app.tab {
            Tab::Service => " Output (c clears) ",
            _ => " Results (c clears) ",
        };
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
            pane,
        );
    }

    let hint_text = match (&app.editing, &app.confirm, rows.get(app.cursor())) {
        (Some(_), _, _) => "typing replaces the selected value; ←/→ edit in place, Enter commits, Esc cancels".to_string(),
        (_, Some(_), _) => "y confirms, any other key cancels".to_string(),
        (_, _, Some(r)) if r.dim && app.tab == Tab::Test => format!(
            "{} (not used with {} / {})",
            r.hint, app.cfg.protocol, app.cfg.mode
        ),
        (_, _, Some(r)) => r.hint.to_string(),
        _ => String::new(),
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("   "),
            Span::styled(hint_text, Style::default().fg(Color::DarkGray).italic()),
        ])),
        hint,
    );

    let keys: &[(&str, &str)] = match app.tab {
        Tab::Test => &[
            ("s", "start"),
            ("Enter", "edit / toggle"),
            ("f", "fetch cert"),
            ("w", "save"),
            ("←→", "tab"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Alerts => &[
            ("a", "add webhook"),
            ("d", "delete"),
            ("t", "test"),
            ("f", "fetch cert"),
            ("w", "save"),
            ("←→", "tab"),
            ("?", "help"),
        ],
        Tab::Monitor => &[
            ("a", "add target"),
            ("d", "delete"),
            ("f", "fetch cert"),
            ("v", "verify"),
            ("t", "test"),
            ("w", "save"),
            ("l", "reload"),
            ("←→", "tab"),
        ],
        Tab::Service => &[
            ("i", "install"),
            ("s", "start"),
            ("x", "stop"),
            ("r", "restart"),
            ("u", "uninstall"),
            ("U", "purge"),
            ("R", "refresh"),
            ("←→", "tab"),
        ],
    };
    f.render_widget(Paragraph::new(keys_line(keys)), footer);
}

fn render_header(app: &App, f: &mut Frame, area: Rect) {
    let [brand, tabs, right] = Layout::horizontal([
        Constraint::Length(10),
        Constraint::Min(30),
        Constraint::Length(40),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(Span::styled(
            " nettest ",
            Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
        )),
        brand,
    );
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| {
            let mark = match t {
                Tab::Alerts | Tab::Monitor if app.draft.dirty => "*",
                _ => "",
            };
            Line::from(format!("{}{mark}", t.title()))
        })
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
            .divider(Span::styled(" | ", Style::default().fg(Color::DarkGray))),
        tabs,
    );
    let right_text = match app.tab {
        Tab::Test => format!("target {}", app.cfg.target()),
        Tab::Alerts | Tab::Monitor => format!(
            "{} target(s), {} webhook(s)",
            app.draft.cfg.targets.len(),
            app.draft.cfg.notify.len()
        ),
        Tab::Service => match &app.service.status {
            Some(s) if s.installed => format!("installed, {}", s.state),
            Some(_) => "not installed".to_string(),
            None => String::new(),
        },
    };
    f.render_widget(
        Paragraph::new(Span::styled(right_text, Style::default().fg(Color::DarkGray)))
            .right_aligned(),
        right,
    );
}

/// Draw `rows` with the cursor row highlighted and, while editing, the editor's spans in place
/// of the value. Scrolls so the cursor stays visible.
pub fn render_rows(
    f: &mut Frame,
    area: Rect,
    rows: &[Row],
    cursor: usize,
    editing: Option<&Editor>,
    two_cols: bool,
) {
    let per_col = if two_cols {
        rows.len().div_ceil(2)
    } else {
        rows.len()
    };
    let cols: Vec<Rect> = if two_cols {
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas::<2>(area)
            .to_vec()
    } else {
        vec![area]
    };
    let label_w = 26usize;
    for (ci, col) in cols.iter().enumerate() {
        let start = ci * per_col;
        let end = (start + per_col).min(rows.len());
        let height = col.height as usize;
        // Single column: scroll so the cursor is visible, with one row of context.
        let offset = if two_cols || height == 0 {
            0
        } else {
            let c = cursor.min(rows.len().saturating_sub(1));
            if c + 1 >= height { c + 2 - height } else { 0 }.min(rows.len().saturating_sub(height))
        };
        let mut lines = Vec::with_capacity(height);
        for (idx, row) in rows[start..end].iter().enumerate().skip(offset).take(height.max(1)) {
            let idx = start + idx;
            let selected = idx == cursor;
            if row.kind == FieldKind::Header {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        row.label.clone(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                ]));
                continue;
            }
            if row.kind == FieldKind::Info {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{:<label_w$}", row.label),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw("   "),
                    Span::styled(row.value.clone(), Style::default().fg(Color::Gray)),
                ]));
                continue;
            }
            let marker = match row.kind {
                FieldKind::Toggle => {
                    if row.value == "on" {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                }
                FieldKind::Cycle => "<> ",
                _ => "   ",
            };
            let value_style = match row.kind {
                _ if row.dim => Style::default().fg(Color::DarkGray),
                FieldKind::Toggle if row.value == "on" => Style::default().fg(Color::Green),
                FieldKind::Toggle => Style::default().fg(Color::DarkGray),
                _ => Style::default().fg(Color::White),
            };
            let mut spans = vec![
                Span::styled(
                    if selected { " ▶ " } else { "   " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{:<label_w$}", row.label),
                    if selected {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if row.dim {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(marker, Style::default().fg(Color::DarkGray)),
            ];
            match (row.kind, selected, editing) {
                (FieldKind::Toggle, _, _) => {}
                (_, true, Some(ed)) => spans.extend(ed.spans(value_style)),
                _ => spans.push(Span::styled(row.value.clone(), value_style)),
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), *col);
    }
}
