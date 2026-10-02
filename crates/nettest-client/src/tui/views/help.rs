//! Help overlay (any key closes).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

const HELP: &[&str] = &[
    "Settings screen",
    "  ↑/↓, Tab, j/k    move between fields",
    "  Enter            edit a text field / toggle a switch",
    "  Space            cycle protocol, mode, direction",
    "  s or F5          start the test",
    "  w                save settings to the config file",
    "",
    "Running screen",
    "  x / s / Esc      stop the test (waits for in-flight probes)",
    "  r                write an HTML report from the data so far",
    "  1 / 2 / 3        chart window: last 60 s / 5 min / everything",
    "  c                clear the log panel",
    "",
    "Summary screen",
    "  s                run again with the same settings",
    "  Enter / Esc      back to settings",
    "  r                write the HTML report",
    "",
    "Everywhere:  q quits, Ctrl-C quits, ? shows this help",
    "",
    "Protocols: ws / wss / tcp / udp need nettest-server on the far end.",
    "  ping / connect / sip / http / https test any device (phone, switch, gateway)",
    "  in latency or soak mode; dimmed fields do not apply to the chosen protocol.",
    "",
    "Headless use:  nettest-client --no-tui ws://host:9101 -c 100 -i 200ms",
    "               nettest-client --no-tui sip://10.0.0.7 -m soak -d 8h --csv",
    "Exit codes: 0 ok, 1 thresholds exceeded, 2 connect/auth, 3 usage, 4 io, 5 interrupted",
];

pub fn render(_app: &App, f: &mut Frame, area: Rect) {
    let w = 78.min(area.width.saturating_sub(2));
    let h = (HELP.len() as u16 + 2).min(area.height.saturating_sub(2));
    let [v] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, popup);
    let lines: Vec<Line> = HELP.iter().map(|s| Line::from(*s)).collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help (any key to close) ")
                .style(Style::default().fg(Color::White).bg(Color::Black)),
        ),
        popup,
    );
}
