//! Renders each screen into an off-screen buffer. Run with `--nocapture` to eyeball the layout.

use std::path::PathBuf;

use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::stats::{ChartPoint, LatencySnapshot, RunSummary, SoakSnapshot};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::app::{App, Link, Screen, Tab};
use crate::monitor::config::{NotifyConfig, TargetConfig};
use crate::runner::{RunOutput, Snapshot, StopReason};

fn draw(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn new_app() -> App {
    App::new(
        ClientConfig::default(),
        PathBuf::from("/tmp/x.toml"),
        std::env::temp_dir().join("nettest-tui-test-missing-monitor.toml"),
    )
}

fn sample_snapshot() -> Snapshot {
    let chart: Vec<ChartPoint> = (0..240)
        .map(|i| {
            let t = i as f64 * 0.25;
            ChartPoint {
                t,
                rtt_ms: if i % 37 == 0 {
                    None
                } else {
                    Some(
                        12.0 + 6.0 * ((t / 7.0).sin() + 1.0) + if i % 50 == 0 { 40.0 } else { 0.0 },
                    )
                },
            }
        })
        .collect();
    Snapshot {
        latency: LatencySnapshot {
            sent: 240,
            received: 233,
            lost: 7,
            late: 1,
            duplicates: 0,
            out_of_order: 2,
            failed: 0,
            in_flight: 1,
            loss_pct: 2.92,
            min_ms: 11.9,
            avg_ms: 18.4,
            max_ms: 64.1,
            p50_ms: 17.8,
            p95_ms: 25.3,
            p99_ms: 58.0,
            jitter_ms: 1.7,
            last_ms: Some(14.2),
            elapsed: std::time::Duration::from_secs(60),
        },
        throughput: None,
        soak: Some(SoakSnapshot {
            disconnects: 1,
            reconnects: 1,
            uptime_pct: 98.2,
            longest_outage_ms: 1080,
            current_session_s: 40.0,
            longest_session_s: 40.0,
            connected: true,
            ..Default::default()
        }),
        chart,
        connected: true,
        elapsed_s: 60.0,
    }
}

#[test]
fn form_screen_renders_all_fields() {
    let mut app = new_app();
    let s = draw(&mut app, 130, 40);
    println!("{s}");
    assert!(s.contains("Target host"));
    assert!(s.contains("Output directory"));
    assert!(s.contains("start"));
    let narrow = draw(&mut app, 80, 30);
    assert!(
        narrow.contains("Output directory"),
        "all fields must fit on 80x30"
    );
}

#[test]
fn running_screen_renders_stats_chart_log() {
    let mut app = new_app();
    app.screen = Screen::Running;
    app.link = Link::Connected;
    app.snapshot = sample_snapshot();
    app.run_cfg.mode = TestMode::Soak;
    let s = draw(&mut app, 130, 40);
    println!("{s}");
    assert!(s.contains("sent / received"));
    assert!(s.contains("240 / 233"));
    assert!(s.contains("RTT"));
    assert!(s.contains("Log"));
    assert!(s.contains("connected"));
    assert!(s.contains("disconnects"));
}

#[test]
fn summary_screen_renders() {
    let mut app = new_app();
    app.screen = Screen::Summary;
    app.snapshot = sample_snapshot();
    app.last_output = Some(RunOutput {
        summary: RunSummary {
            latency: app.snapshot.latency.clone(),
            soak: app.snapshot.soak.clone(),
            stop_reason: "completed".into(),
            mode: "latency".into(),
            protocol: Some(nettest_proto::transport::Protocol::Ws),
            ..Default::default()
        },
        reason: StopReason::Completed,
        chart: app.snapshot.chart.clone(),
        events: vec![],
    });
    app.report_path = Some(PathBuf::from("./nettest-20260101-000000.html"));
    let s = draw(&mut app, 130, 40);
    println!("{s}");
    assert!(s.contains("run finished"));
    assert!(s.contains("rtt p50/p95/p99"));
    assert!(s.contains("nettest-20260101-000000.html"));
}

#[test]
fn settings_tabs_render() {
    let mut app = new_app();
    app.draft.cfg.targets.push(TargetConfig {
        name: "pbx".into(),
        target: "wss://10.0.0.7:9101".into(),
        ..Default::default()
    });
    app.draft.cfg.notify.push(NotifyConfig {
        url: "https://ntfy.sh/ops".into(),
        ..Default::default()
    });

    app.tab = Tab::Alerts;
    let s = draw(&mut app, 100, 40);
    println!("{s}");
    assert!(s.contains("Send alerts during runs"));
    assert!(s.contains("https://ntfy.sh/ops"));
    assert!(s.contains("Alerts"));

    app.tab = Tab::Monitor;
    let s = draw(&mut app, 100, 40);
    println!("{s}");
    assert!(s.contains("[[targets]] #1  pbx"));
    assert!(s.contains("wss://10.0.0.7:9101"));
    assert!(s.contains("1 webhook(s)"));
    // Narrow terminal: the list scrolls instead of overflowing.
    let narrow = draw(&mut app, 80, 24);
    assert!(narrow.contains("monitor.toml path"));

    app.tab = Tab::Service;
    app.service.status = Some(crate::service::ServiceStatus {
        elevated: false,
        user: "gabriel".into(),
        supported: true,
        installed: true,
        state: "active".into(),
        paths: crate::service::service_paths(),
        config: Some(Ok(vec!["targets     1".into()])),
    });
    let s = draw(&mut app, 100, 40);
    println!("{s}");
    assert!(s.contains("user gabriel"));
    assert!(s.contains("installed, active"));
    assert!(s.contains("install from"));
}

#[test]
fn editing_highlights_the_whole_value() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let mut app = new_app();
    rt.block_on(app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let ed = app.editing.as_ref().expect("editing the host");
    assert!(ed.selected());
    assert_eq!(ed.text(), "127.0.0.1");
    rt.block_on(app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)));
    assert_eq!(app.editing.as_ref().unwrap().text(), "h");
    rt.block_on(app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.editing.is_none());
    assert_eq!(app.cfg.host, "h");

    // Cursor movement skips headers on the dynamic tabs and wraps.
    rt.block_on(app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)));
    assert_eq!(app.tab, Tab::Alerts);
    assert!(app.rows()[app.cursor()].kind.selectable());
    rt.block_on(app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
    assert!(app.rows()[app.cursor()].kind.selectable());
}
