//! Help overlay (any key closes).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

const HELP: &[&str] = &[
    "Settings tabs (Test | Alerts | Monitor | Service):  ←/→ or F1-F4 switch tabs",
    "  ↑/↓, Tab, j/k    move between fields        Enter   edit a text field / flip a switch",
    "  Space            cycle a <> field            typing replaces the selected value;",
    "                                               ←/→ edit in place, Enter commits, Esc cancels",
    "Test:     s or F5 start   w save client.toml   f fetch the server certificate's SHA-256",
    "Alerts:   a add webhook   d delete   t send a test message   f fetch cert   w save",
    "          webhooks live in monitor.toml ([[notify]]), shared with the monitor service;",
    "          DOWN / UP rules for runs started here live in client.toml",
    "Monitor:  a add target    d delete   f fetch cert   v verify (like monitor --check)",
    "          t test webhooks  w write monitor.toml   l reload from disk",
    "Service:  i install from the path shown   s start   x stop   r restart   u uninstall",
    "          U uninstall --purge   R refresh status   (sudo / UAC prompt when not root)",
    "",
    "Running:  x / s / Esc stop   r HTML report now   1/2/3 chart window   c clear log",
    "Summary:  s run again   Enter / Esc back to settings   r write the HTML report",
    "Everywhere:  q quits, Ctrl-C quits, ? shows this help",
    "",
    "Protocols: ws / wss / tcp / udp need nettest-server on the far end; ping / connect / sip /",
    "  http / https test any device in latency or soak mode. Dimmed fields do not apply.",
    "Headless:  nettest-client --no-tui ws://host:9101 -c 100 -i 200ms",
    "Exit codes: 0 ok, 1 thresholds exceeded, 2 connect/auth, 3 usage, 4 io, 5 interrupted",
];

pub fn render(_app: &App, f: &mut Frame, area: Rect) {
    let w = 96.min(area.width.saturating_sub(2));
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
