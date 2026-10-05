//! Monitor supervisor against an in-process nettest-server: DOWN when the server goes away, UP
//! when it is back, for a persistent-connection target (ws) and a serverless one (connect),
//! plus webhook delivery against a local fake endpoint.

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
