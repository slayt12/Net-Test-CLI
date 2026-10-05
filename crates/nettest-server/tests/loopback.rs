//! In-process loopback tests: a real server on port 0, a real client transport, every protocol.

use std::time::Duration;

use nettest_proto::bytes::Bytes;
use nettest_proto::config::ServerConfig;
use nettest_proto::frame::payloads::{TpResultPayload, TpStartPayload, from_json, to_json};
use nettest_proto::frame::{HEADER_LEN, Header, Kind, flags};
use nettest_proto::time::now_ns;
use nettest_proto::tls::client::TlsClientMode;
use nettest_proto::transport::{DialError, DialOptions, Protocol, dial};
use nettest_server::log::{LogOptions, ServerLog};
use nettest_server::{RunningServer, start};

static TEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn spawn(token: &str) -> RunningServer {
    // Tests run concurrently in one process; each needs its own cert dir or they race on rcgen.
    let n = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("nettest-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (log, _tail) = ServerLog::start(LogOptions {
        stderr: false,
        text_file: None,
        jsonl_file: None,
        tail_capacity: 50,
    })
    .unwrap();
    let cfg = ServerConfig {
        bind: "127.0.0.1".into(),
        tcp_port: 0,
        udp_port: 0,
        ws_port: 0,
        wss_port: 0,
        token: token.into(),
        cert_dir: dir.display().to_string(),
        idle_timeout_secs: 5,
        ..Default::default()
    };
    // Port 0 means "disabled" in config, so bind explicit ephemeral ports instead.
    let mut cfg = cfg;
    cfg.tcp_port = free_port();
    cfg.udp_port = free_port();
    cfg.ws_port = free_port();
    cfg.wss_port = free_port();
    start(cfg, log).await.expect("server starts")
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn opts(server: &RunningServer, p: Protocol, token: Option<&str>) -> DialOptions {
    let addr = server.addr(p).expect("listener bound");
    DialOptions {
        host: addr.ip().to_string(),
        port: addr.port(),
        protocol: p,
        tls: p
            .is_tls()
            .then(|| TlsClientMode::Pinned(server.fingerprint.unwrap())),
        token: token.map(str::to_string),
        client_id: "test".into(),
        mode: "latency".into(),
        connect_timeout: Duration::from_secs(5),
        prefer_ipv6: None,
        bind: None,
        ws_path: "/".into(),
    }
}

async fn echo_round(server: &RunningServer, p: Protocol) {
    let (mut t, timings) = dial(&opts(server, p, None)).await.expect("dial");
    assert_eq!(timings.protocol, Some(p));
    assert!(timings.hello.is_some());
    if p == Protocol::Wss {
        assert!(timings.tls.is_some());
    }
    for seq in 1..=50u64 {
        let payload = Bytes::from(vec![seq as u8; 100]);
        t.send(
            Header::new(Kind::Probe)
                .with_seq(seq)
                .with_send_ns(now_ns()),
            payload.clone(),
        )
        .await
        .unwrap();
        let f = tokio::time::timeout(Duration::from_secs(2), t.recv())
            .await
            .expect("echo in time")
            .unwrap();
        assert_eq!(f.header.kind, Kind::Echo);
        assert_eq!(f.header.seq, seq);
        assert_eq!(f.payload, payload);
        assert!(now_ns() >= f.header.client_send_ns);
    }
    t.send(Header::new(Kind::Bye), Bytes::new()).await.unwrap();
    let _ = t.close().await;
}

#[tokio::test]
async fn echo_all_protocols() {
    let server = spawn("").await;
    for p in [Protocol::Tcp, Protocol::Udp, Protocol::Ws, Protocol::Wss] {
        echo_round(&server, p).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(server.ctx.sessions.total_accepted() >= 4);
    server.shutdown();
}

#[tokio::test]
async fn token_rejects_and_accepts() {
    let server = spawn("secret").await;
    for p in [Protocol::Tcp, Protocol::Udp, Protocol::Ws] {
        let err = dial(&opts(&server, p, Some("wrong")))
            .await
            .err()
            .expect("rejected");
        assert!(
            matches!(err, DialError::Rejected(ref r) if r == "auth"),
            "{p}: {err}"
        );
        let err = dial(&opts(&server, p, None)).await.err().expect("rejected");
        assert!(matches!(err, DialError::Rejected(_)), "{p}: {err}");
        let (mut t, _) = dial(&opts(&server, p, Some("secret")))
            .await
            .expect("accepted");
        let _ = t.close().await;
    }
    server.shutdown();
}

#[tokio::test]
async fn wrong_fingerprint_fails_tls() {
    let server = spawn("").await;
    let mut o = opts(&server, Protocol::Wss, None);
    o.tls = Some(TlsClientMode::Pinned([0u8; 32]));
    let err = dial(&o).await.err().expect("tls must fail");
    assert!(matches!(err, DialError::Tls(_)), "{err}");
    o.tls = Some(TlsClientMode::Insecure);
    dial(&o).await.expect("insecure accepts");
    server.shutdown();
}

/// The monitor's webhook mode must never accept a nettest-server's self-signed certificate.
#[tokio::test]
async fn webpki_rejects_self_signed() {
    let server = spawn("").await;
    let mut o = opts(&server, Protocol::Wss, None);
    o.tls = Some(TlsClientMode::WebPki);
    let err = dial(&o).await.err().expect("tls must fail");
    assert!(matches!(err, DialError::Tls(_)), "{err}");
    server.shutdown();
}

#[tokio::test]
async fn throughput_upload_and_download_tcp() {
    let server = spawn("").await;
    let (mut t, _) = dial(&opts(&server, Protocol::Tcp, None)).await.unwrap();
    let chunk = 4096u32;
    t.send(
        Header::new(Kind::TpStart),
        to_json(&TpStartPayload { secs: 1, chunk }).into(),
    )
    .await
    .unwrap();
    let payload = Bytes::from(vec![1u8; chunk as usize]);
    for seq in 0..200u64 {
        t.send(Header::new(Kind::TpData).with_seq(seq), payload.clone())
            .await
            .unwrap();
    }
    t.send(Header::new(Kind::TpEnd).with_seq(200), Bytes::new())
        .await
        .unwrap();
    let f = tokio::time::timeout(Duration::from_secs(5), t.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.header.kind, Kind::TpResult);
    let res: TpResultPayload = from_json(&f.payload).unwrap();
    assert_eq!(res.frames, 200);
    assert_eq!(res.bytes, 200 * (HEADER_LEN as u64 + chunk as u64));

    t.send(
        Header::new(Kind::TpStart).with_flags(flags::TP_DOWNLOAD),
        to_json(&TpStartPayload { secs: 1, chunk }).into(),
    )
    .await
    .unwrap();
    let mut frames = 0u64;
    loop {
        let f = tokio::time::timeout(Duration::from_secs(5), t.recv())
            .await
            .unwrap()
            .unwrap();
        match f.header.kind {
            Kind::TpData => frames += 1,
            Kind::TpEnd => {
                assert_eq!(f.header.seq, frames);
                break;
            }
            k => panic!("unexpected {k}"),
        }
    }
    assert!(frames > 10);
    server.shutdown();
}

#[tokio::test]
async fn unauthenticated_stream_times_out_and_udp_strangers_are_ignored() {
    let server = spawn("").await;
    let udp = server.addr(Protocol::Udp).unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let probe =
        nettest_proto::frame::Frame::new(Header::new(Kind::Probe).with_seq(1), vec![0u8; 8]);
    sock.send_to(&probe.to_bytes(), udp).await.unwrap();
    let mut buf = [0u8; 128];
    let r = tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await;
    assert!(r.is_err(), "server must not reflect to unknown UDP peers");
    server.shutdown();
}

const BANNER_TEXT: &str = "nettest by Slaytons Technology Services";

async fn read_all(stream: &mut tokio::net::TcpStream, within: Duration) -> String {
    use tokio::io::AsyncReadExt;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(within, stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn scanners_get_a_banner_on_every_stream_port() {
    use tokio::io::AsyncWriteExt;
    let server = spawn("").await;

    // HTTP on the raw TCP port: an HTTP response carrying the banner.
    let tcp = server.addr(Protocol::Tcp).unwrap();
    let mut s = tokio::net::TcpStream::connect(tcp).await.unwrap();
    s.write_all(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let text = read_all(&mut s, Duration::from_secs(2)).await;
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(text.contains(BANNER_TEXT), "{text}");

    // Silence (nmap null probe) on the TCP port: a bare banner line within the wait.
    let mut s = tokio::net::TcpStream::connect(tcp).await.unwrap();
    let text = read_all(&mut s, Duration::from_secs(5)).await;
    assert!(text.starts_with(BANNER_TEXT), "{text}");

    // Random bytes on the ws port: bare banner. Plain GET on the ws port: HTTP banner.
    let ws = server.addr(Protocol::Ws).unwrap();
    let mut s = tokio::net::TcpStream::connect(ws).await.unwrap();
    s.write_all(&[0x16, 0x03, 0x01, 0x00, 0x05, 1, 2, 3])
        .await
        .unwrap();
    let text = read_all(&mut s, Duration::from_secs(2)).await;
    assert!(text.starts_with(BANNER_TEXT), "{text}");
    let mut s = tokio::net::TcpStream::connect(ws).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let text = read_all(&mut s, Duration::from_secs(2)).await;
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");

    // wss: TLS handshake with any-cert policy, then a plain GET inside the tunnel.
    let wss = server.addr(Protocol::Wss).unwrap();
    let cfg = nettest_proto::tls::client::client_config(TlsClientMode::Insecure).unwrap();
    let tcp_stream = tokio::net::TcpStream::connect(wss).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1".to_string()).unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(cfg)
        .connect(name, tcp_stream)
        .await
        .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::io::AsyncReadExt::read_to_end(&mut tls, &mut out),
    )
    .await;
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.starts_with("HTTP/1.1 200 OK") && text.contains(BANNER_TEXT),
        "{text}"
    );

    // Real clients still work after all that.
    echo_round(&server, Protocol::Tcp).await;
    echo_round(&server, Protocol::Ws).await;
    server.shutdown();
}

#[tokio::test]
async fn udp_junk_gets_a_rate_limited_banner() {
    let server = spawn("").await;
    let udp = server.addr(Protocol::Udp).unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.send_to(b"hello?", udp).await.unwrap();
    let mut buf = [0u8; 256];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
        .await
        .expect("banner datagram")
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with(BANNER_TEXT));
    // Second junk datagram from the same source within 2 s: no reply (reflection budget).
    sock.send_to(b"hello again?", udp).await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await;
    assert!(r.is_err(), "banner must be rate limited per source");
    echo_round(&server, Protocol::Udp).await;
    server.shutdown();
}
