//! Monitor supervisor against an in-process nettest-server: DOWN when the server goes away, UP
//! when it is back, for a persistent-connection target (ws) and a serverless one (connect),
//! plus webhook delivery (Discord and Teams payloads) against local fake endpoints.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nettest_client::monitor::config::{NotifyKind, ResolvedTarget, ResolvedNotifier};
use nettest_client::monitor::health::{Thresholds, Transition};
use nettest_client::monitor::notify::{self, Kind, Notification};
use nettest_client::monitor::supervisor::{TargetEvent, TargetStatus, run_target};
use nettest_proto::config::{ClientConfig, ServerConfig, SinkConfig, TestMode};
use nettest_proto::http::Url;
use nettest_proto::log::{LineLog, LogOptions};
use nettest_proto::tls::client::TlsClientMode;
use nettest_proto::transport::Protocol;
use nettest_server::RunningServer;
use nettest_server::log::{LogOptions as ServerLogOptions, ServerLog};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

static TEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Ports {
    tcp: u16,
    udp: u16,
    ws: u16,
    wss: u16,
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn ports() -> Ports {
    Ports {
        tcp: free_port(),
        udp: free_port(),
        ws: free_port(),
        wss: free_port(),
    }
}

async fn spawn_on(p: &Ports) -> RunningServer {
    // Tests run concurrently in one process; each server needs its own cert dir (rcgen races).
    let n = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nettest-mon-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (log, _tail) = ServerLog::start(ServerLogOptions {
        stderr: false,
        text_file: None,
        jsonl_file: None,
        tail_capacity: 50,
    })
    .unwrap();
    let cfg = ServerConfig {
        bind: "127.0.0.1".into(),
        tcp_port: p.tcp,
        udp_port: p.udp,
        ws_port: p.ws,
        wss_port: p.wss,
        token: String::new(),
        cert_dir: dir.display().to_string(),
        idle_timeout_secs: 5,
        ..Default::default()
    };
    nettest_server::start(cfg, log).await.expect("server starts")
}

fn target(protocol: Protocol, port: u16, down_after: u32) -> ResolvedTarget {
    let client = ClientConfig {
        host: "127.0.0.1".into(),
        port,
        protocol,
        mode: TestMode::Latency,
        interval_ms: 100,
        connect_timeout_secs: 1,
        loss_timeout_ms: 1000,
        outage_after: down_after,
        sinks: SinkConfig {
            console: false,
            html_report: false,
            ..Default::default()
        },
        ..Default::default()
    };
    ResolvedTarget {
        name: format!("{protocol}-target"),
        client,
        interval: Duration::from_millis(100),
        timeout: Duration::from_secs(1),
        thresholds: Thresholds {
            down_after,
            up_after: 1,
            remind_every: None,
        },
    }
}

fn quiet_log() -> LineLog {
    LineLog::start(LogOptions {
        stderr: std::env::var_os("NETTEST_TEST_LOG").is_some(),
        text_file: None,
        jsonl_file: None,
        tail_capacity: 1,
    })
    .unwrap()
    .0
}

async fn expect(
    rx: &mut mpsc::Receiver<TargetEvent>,
    what: &str,
    secs: u64,
    pred: impl Fn(&Transition) -> bool,
) -> Transition {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no {what} within {secs} s"))
            .expect("supervisor alive");
        if pred(&ev.transition) {
            return ev.transition;
        }
    }
}

#[tokio::test]
async fn ws_target_down_and_up() {
    let p = ports();
    let server = spawn_on(&p).await;
    let (tx, mut rx) = mpsc::channel(64);
    let status = Arc::new(Mutex::new(TargetStatus::default()));
    let cancel = CancellationToken::new();
    let task = tokio::spawn(run_target(
        0,
        target(Protocol::Ws, p.ws, 3),
        tx,
        status.clone(),
        quiet_log(),
        cancel.clone(),
    ));

    expect(&mut rx, "initial up", 10, |t| {
        matches!(t, Transition::Initial { up: true, .. })
    })
    .await;
    server.shutdown();
    drop(server);
    let down = expect(&mut rx, "DOWN", 15, |t| matches!(t, Transition::Down { .. })).await;
    assert!(matches!(down, Transition::Down { consecutive: 3, .. }), "{down:?}");

    let server2 = spawn_on(&p).await;
    let up = expect(&mut rx, "UP", 40, |t| matches!(t, Transition::Up { .. })).await;
    assert!(matches!(up, Transition::Up { .. }));
    assert!(status.lock().unwrap().last_snapshot.is_some());

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("supervisor stops on cancel")
        .unwrap();
    server2.shutdown();
}

#[tokio::test]
async fn connect_target_restarts_runner() {
    let p = ports();
    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(run_target(
        0,
        target(Protocol::Connect, p.tcp, 3),
        tx,
        Arc::new(Mutex::new(TargetStatus::default())),
        quiet_log(),
        cancel.clone(),
    ));
    // Nothing listens yet: refused probes make the target DOWN from the start.
    let first = expect(&mut rx, "initial down", 15, |t| {
        matches!(t, Transition::Initial { up: false, .. })
    })
    .await;
    assert!(matches!(first, Transition::Initial { ref reason, .. } if reason.contains("refused")), "{first:?}");

    let server = spawn_on(&p).await;
    expect(&mut rx, "UP after restart", 30, |t| matches!(t, Transition::Up { .. })).await;

    server.shutdown();
    drop(server);
    expect(&mut rx, "DOWN again", 15, |t| matches!(t, Transition::Down { consecutive: 3, .. })).await;

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("supervisor stops on cancel")
        .unwrap();
}

#[tokio::test]
async fn webhook_delivery_against_local_listener() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut got = Vec::new();
        loop {
            let n = s.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") && got.ends_with(b"}") {
                break;
            }
        }
        s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        s.shutdown().await.unwrap();
        String::from_utf8_lossy(&got).into_owned()
    });
    let n = ResolvedNotifier {
        kind: NotifyKind::Discord,
        name: "fake discord".into(),
        url: Url::parse(&format!("http://127.0.0.1:{port}/api/webhooks/1/x")).unwrap(),
        token: String::new(),
        priority: "high".into(),
        tags_down: String::new(),
        tags_up: String::new(),
        tls: TlsClientMode::WebPki,
        timeout: Duration::from_secs(5),
    };
    let msg = Notification {
        kind: Kind::Down,
        title: "nettest: pbx DOWN".into(),
        body: "pbx (sip://10.0.0.5:5060) is DOWN".into(),
    };
    let failed = notify::deliver_all(&[n], &msg, &quiet_log()).await;
    assert_eq!(failed, 0);
    let req = server.await.unwrap();
    assert!(req.starts_with("POST /api/webhooks/1/x HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("Content-Type: application/json\r\n"), "{req}");
    let body = req.split("\r\n\r\n").nth(1).unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert!(v["content"].as_str().unwrap().contains("**nettest: pbx DOWN**"));
    assert_eq!(v["username"], "nettest");
}

/// Teams Workflows trigger: the query string (which carries the signature) must reach the
/// server untouched, the body must be the `message` + Adaptive Card envelope, and the trigger's
/// `202 Accepted` counts as delivered.
#[tokio::test]
async fn teams_delivery_against_local_listener() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let mut got = Vec::new();
        loop {
            let n = s.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
            if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") && got.ends_with(b"}") {
                break;
            }
        }
        s.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        s.shutdown().await.unwrap();
        String::from_utf8_lossy(&got).into_owned()
    });
    let path = "/powerautomate/automations/direct/cu/1/workflows/2/triggers/manual/paths/invoke?api-version=1&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=abc";
    let n = ResolvedNotifier {
        kind: NotifyKind::Teams,
        name: "fake teams".into(),
        url: Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap(),
        token: String::new(),
        priority: "high".into(),
        tags_down: String::new(),
        tags_up: String::new(),
        tls: TlsClientMode::WebPki,
        timeout: Duration::from_secs(5),
    };
    let msg = Notification {
        kind: Kind::Up,
        title: "nettest: pbx UP".into(),
        body: "pbx (sip://10.0.0.5:5060) is UP again".into(),
    };
    let failed = notify::deliver_all(&[n], &msg, &quiet_log()).await;
    assert_eq!(failed, 0);
    let req = server.await.unwrap();
    assert!(req.starts_with(&format!("POST {path} HTTP/1.1\r\n")), "{req}");
    assert!(req.contains("Content-Type: application/json\r\n"), "{req}");
    let body = req.split("\r\n\r\n").nth(1).unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["type"], "message");
    assert!(v["text"].as_str().unwrap().starts_with("nettest: pbx UP\n"));
    let card = &v["attachments"][0];
    assert_eq!(card["contentType"], "application/vnd.microsoft.card.adaptive");
    assert_eq!(card["content"]["body"][0]["text"], "nettest: pbx UP");
    assert_eq!(card["content"]["body"][0]["color"], "Good");
}

/// The TUI's certificate fetch must return the fingerprint the server itself reports.
#[tokio::test]
async fn peer_fingerprint_matches_the_server_certificate() {
    let p = ports();
    let server = spawn_on(&p).await;
    let expected = server.fingerprint.expect("wss listener has a certificate");
    let (got, addr) = nettest_proto::tls::client::peer_fingerprint(
        "127.0.0.1",
        p.wss,
        None,
        Duration::from_secs(5),
    )
    .await
    .expect("fetch");
    assert_eq!(got, expected);
    assert_eq!(addr.port(), p.wss);
    // A non-TLS port must fail with a handshake error, not hang or panic.
    let err = nettest_proto::tls::client::peer_fingerprint(
        "127.0.0.1",
        p.ws,
        None,
        Duration::from_secs(3),
    )
    .await
    .unwrap_err();
    assert!(err.contains("tls handshake"), "{err}");
    server.shutdown();
}

/// Alerts for an interactive run: synthetic runner events drive the same DOWN / UP rules as
/// the monitor and the messages reach a local ntfy-style listener.
#[tokio::test]
async fn tui_run_alerts_reach_a_local_listener() {
    use nettest_client::runner::UiEvent;
    use nettest_client::tui::App;
    use nettest_proto::transport::ConnectTimings;

    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let listener = tokio::spawn(async move {
        let mut titles = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).into_owned();
                if n == 0 || text.contains("\r\n\r\n") && text.contains("DOWN") || text.contains("UP again") {
                    break;
                }
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
            s.shutdown().await.unwrap();
            let text = String::from_utf8_lossy(&got).into_owned();
            let title = text
                .lines()
                .find_map(|l| l.strip_prefix("Title: "))
                .unwrap_or("")
                .to_string();
            titles.push(title);
        }
        titles
    });

    let mut cfg = ClientConfig {
        host: "10.0.0.7".into(),
        port: 9101,
        protocol: Protocol::Ws,
        mode: TestMode::Soak,
        ..Default::default()
    };
    cfg.alerts.failures_before_down = 1;
    cfg.alerts.successes_before_up = 1;
    cfg.alerts.hostname = "lab".into();
    let mut app = App::new(
        cfg,
        std::env::temp_dir().join("nettest-tui-alerts-client.toml"),
        std::env::temp_dir().join("nettest-tui-alerts-missing-monitor.toml"),
    );
    app.draft.cfg.notify.push(nettest_client::monitor::config::NotifyConfig {
        url: format!("http://127.0.0.1:{port}/ops"),
        ..Default::default()
    });
    app.start_for_test();
    assert!(app.is_running_alerts(), "alerts should be armed");

    app.on_runner_event(UiEvent::Connected(ConnectTimings::default())).await;
    app.on_runner_event(UiEvent::Disconnected("connection reset".into())).await;
    app.on_runner_event(UiEvent::Connected(ConnectTimings::default())).await;

    let titles = tokio::time::timeout(Duration::from_secs(10), listener)
        .await
        .expect("both alerts delivered")
        .unwrap();
    assert_eq!(titles[0], "nettest: ws://10.0.0.7:9101 DOWN");
    assert_eq!(titles[1], "nettest: ws://10.0.0.7:9101 UP");

    // Delivery outcomes come back over the background channel and land in the run log.
    for _ in 0..2 {
        let ev = tokio::time::timeout(Duration::from_secs(5), app.bg_rx.recv())
            .await
            .expect("delivery report")
            .unwrap();
        app.on_bg(ev).await;
    }
    let log: Vec<String> = app.log.iter().map(|l| l.text.clone()).collect();
    assert!(log.iter().any(|l| l.contains("alert: nettest: ws://10.0.0.7:9101 DOWN")), "{log:?}");
    assert_eq!(log.iter().filter(|l| l.contains("delivered")).count(), 2, "{log:?}");
}
