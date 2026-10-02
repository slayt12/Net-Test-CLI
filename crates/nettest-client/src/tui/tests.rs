//! Renders each screen into an off-screen buffer. Run with `--nocapture` to eyeball the layout.

use std::path::PathBuf;

use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::stats::{ChartPoint, LatencySnapshot, RunSummary, SoakSnapshot};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::app::{App, Link, Screen};
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
    let mut app = App::new(ClientConfig::default(), PathBuf::from("/tmp/x.toml"));
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
    let mut app = App::new(ClientConfig::default(), PathBuf::from("/tmp/x.toml"));
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
    let mut app = App::new(ClientConfig::default(), PathBuf::from("/tmp/x.toml"));
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
