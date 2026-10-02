//! Client-side connect pipeline with per-phase timing.
//!
//! DNS -> TCP connect -> (TLS) -> (WebSocket upgrade) -> Hello/HelloAck. Each phase is timed
//! separately because "the network is slow" is almost always one specific phase.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::{TcpStream, UdpSocket, lookup_host};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::rewind::Rewind;
use super::ws::MaybeTls;
use super::{AnyTransport, ConnectTimings, Protocol, TransportError};
use crate::frame::payloads::{HelloAckPayload, HelloPayload, from_json, to_json};
use crate::frame::{Frame, Header, Kind};
use crate::tls::client::TlsClientMode;

#[derive(Debug, Clone)]
pub struct DialOptions {
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    pub tls: Option<TlsClientMode>,
    pub token: Option<String>,
    pub client_id: String,
    pub mode: String,
    pub connect_timeout: Duration,
    /// `Some(true)` = only IPv6 results, `Some(false)` = only IPv4, `None` = first answer.
    pub prefer_ipv6: Option<bool>,
    pub bind: Option<IpAddr>,
    /// WebSocket request path (default `/`).
    pub ws_path: String,
}

impl Default for DialOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 9100,
            protocol: Protocol::Ws,
            tls: None,
            token: None,
            client_id: "nettest".into(),
            mode: "latency".into(),
            connect_timeout: Duration::from_secs(5),
            prefer_ipv6: None,
            bind: None,
            ws_path: "/".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("dns lookup for {host}: {source}")]
    Dns {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no address found for {0}")]
    NoAddress(String),
    #[error("timed out after {0:?} during {1}")]
    Timeout(Duration, &'static str),
    #[error("tcp connect to {addr}: {source}")]
    Tcp {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("udp socket: {0}")]
    Udp(#[source] std::io::Error),
    #[error("tls handshake: {0}")]
    Tls(#[source] std::io::Error),
    #[error("tls config: {0}")]
    TlsConfig(#[from] rustls::Error),
    #[error("wss requires --insecure or --fingerprint")]
    TlsModeMissing,
    #[error("invalid server name '{0}'")]
    ServerName(String),
    #[error("websocket upgrade: {0}")]
    WsUpgrade(#[source] tokio_tungstenite::tungstenite::Error),
    #[error("server rejected hello: {0}")]
    Rejected(String),
    #[error("handshake: {0}")]
    Handshake(#[from] TransportError),
    #[error("{0} is a serverless probe, not a nettest-server protocol")]
    Unsupported(Protocol),
}

/// Resolve `host` to one address, honouring the v4/v6 preference. Literal IPs skip DNS and
/// report `None` for the DNS phase so the timing is honest about what it measured.
pub async fn resolve(
    host: &str,
    port: u16,
    prefer_ipv6: Option<bool>,
    timeout: Duration,
) -> Result<(SocketAddr, Option<Duration>), DialError> {
    let bare = host.trim_matches(['[', ']']);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok((SocketAddr::new(ip, port), None));
    }
    let t0 = Instant::now();
    let fut = lookup_host((bare, port));
    let addrs: Vec<SocketAddr> = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| DialError::Timeout(timeout, "dns"))?
        .map_err(|source| DialError::Dns {
            host: host.to_string(),
            source,
        })?
        .collect();
    let dns = t0.elapsed();
    let pick = addrs
        .iter()
        .find(|a| match prefer_ipv6 {
            Some(true) => a.is_ipv6(),
            Some(false) => a.is_ipv4(),
            None => true,
        })
        .copied()
        .ok_or_else(|| DialError::NoAddress(host.to_string()))?;
    Ok((pick, Some(dns)))
}

/// Resolve, connect, authenticate. Returns the ready transport and the timing breakdown.
pub async fn dial(opts: &DialOptions) -> Result<(AnyTransport, ConnectTimings), DialError> {
    let start = Instant::now();
    let mut t = ConnectTimings {
        protocol: Some(opts.protocol),
        ..Default::default()
    };

    let (addr, dns) = resolve(
        &opts.host,
        opts.port,
        opts.prefer_ipv6,
        opts.connect_timeout,
    )
    .await?;
    t.dns = dns;
    t.peer = Some(addr);

    let mut transport = match opts.protocol {
        Protocol::Udp => {
            let bind = opts
                .bind
                .map(|ip| SocketAddr::new(ip, 0))
                .unwrap_or(match addr {
                    SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
                    SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
                });
            let sock = UdpSocket::bind(bind).await.map_err(DialError::Udp)?;
            sock.connect(addr).await.map_err(DialError::Udp)?;
            AnyTransport::Udp(super::udp::UdpTransport::new(sock))
        }
        Protocol::Tcp | Protocol::Ws | Protocol::Wss => {
            let t0 = Instant::now();
            let stream = tokio::time::timeout(opts.connect_timeout, connect_tcp(addr, opts.bind))
                .await
                .map_err(|_| DialError::Timeout(opts.connect_timeout, "tcp connect"))?
                .map_err(|source| DialError::Tcp { addr, source })?;
            t.tcp = Some(t0.elapsed());
            let _ = stream.set_nodelay(true);

            if opts.protocol == Protocol::Tcp {
                AnyTransport::Tcp(super::tcp::TcpTransport::new(stream))
            } else {
                let stream = if opts.protocol == Protocol::Wss {
                    let mode = opts.tls.ok_or(DialError::TlsModeMissing)?;
                    let cfg = crate::tls::client::client_config(mode)?;
                    let name = rustls_pki_types::ServerName::try_from(
                        opts.host.trim_matches(['[', ']']).to_string(),
                    )
                    .map_err(|_| DialError::ServerName(opts.host.clone()))?;
                    let t0 = Instant::now();
                    let tls = tokio_rustls::TlsConnector::from(cfg);
                    let s = tokio::time::timeout(opts.connect_timeout, tls.connect(name, stream))
                        .await
                        .map_err(|_| DialError::Timeout(opts.connect_timeout, "tls"))?
                        .map_err(DialError::Tls)?;
                    t.tls = Some(t0.elapsed());
                    MaybeTls::ClientTls(Box::new(s))
                } else {
                    MaybeTls::Plain(stream)
                };

                let scheme = if opts.protocol == Protocol::Wss {
                    "wss"
                } else {
                    "ws"
                };
                let host_for_url = if addr.is_ipv6() && opts.host.parse::<IpAddr>().is_ok() {
                    format!("[{}]", opts.host.trim_matches(['[', ']']))
                } else {
                    opts.host.clone()
                };
                let url = format!(
                    "{scheme}://{host_for_url}:{}{}",
                    opts.port,
                    if opts.ws_path.starts_with('/') {
                        opts.ws_path.clone()
                    } else {
                        format!("/{}", opts.ws_path)
                    }
                );
                let req = url.into_client_request().map_err(DialError::WsUpgrade)?;
                let t0 = Instant::now();
                let (ws, _resp) = tokio::time::timeout(
                    opts.connect_timeout,
                    tokio_tungstenite::client_async(req, Rewind::new(stream, bytes::Bytes::new())),
                )
                .await
                .map_err(|_| DialError::Timeout(opts.connect_timeout, "websocket upgrade"))?
                .map_err(DialError::WsUpgrade)?;
                t.ws_upgrade = Some(t0.elapsed());
                AnyTransport::Ws(super::ws::WsTransport::new(ws))
            }
        }
        other => return Err(DialError::Unsupported(other)),
    };

    // Hello / HelloAck. UDP retries because the first datagram may legitimately vanish.
    let hello = HelloPayload {
        token: opts.token.clone(),
        client_id: opts.client_id.clone(),
        mode: opts.mode.clone(),
    };
    let hello_frame = Frame::new(Header::new(Kind::Hello), to_json(&hello));
    let attempts = if opts.protocol == Protocol::Udp { 5 } else { 1 };
    let per_attempt = if opts.protocol == Protocol::Udp {
        Duration::from_millis(500).min(opts.connect_timeout)
    } else {
        opts.connect_timeout
    };
    let t0 = Instant::now();
    let mut last_err = None;
    for _ in 0..attempts {
        transport.send_frame(hello_frame.clone()).await?;
        match tokio::time::timeout(per_attempt, transport.recv()).await {
            Ok(Ok(f)) if f.header.kind == Kind::HelloAck => {
                let _ack: HelloAckPayload = from_json(&f.payload).unwrap_or_default();
                last_err = None;
                break;
            }
            Ok(Ok(f)) if f.header.kind == Kind::Error => {
                let reason = String::from_utf8_lossy(&f.payload).into_owned();
                return Err(DialError::Rejected(reason));
            }
            Ok(Ok(_)) => {
                last_err = Some(DialError::Rejected("unexpected frame during hello".into()));
            }
            Ok(Err(e)) => return Err(DialError::Handshake(e)),
            Err(_) => last_err = Some(DialError::Timeout(per_attempt, "hello")),
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }
    t.hello = Some(t0.elapsed());
    t.total = start.elapsed();
    Ok((transport, t))
}

pub(crate) async fn connect_tcp(
    addr: SocketAddr,
    bind: Option<IpAddr>,
) -> std::io::Result<TcpStream> {
    match bind {
        None => TcpStream::connect(addr).await,
        Some(ip) => {
            let sock = if addr.is_ipv4() {
                tokio::net::TcpSocket::new_v4()?
            } else {
                tokio::net::TcpSocket::new_v6()?
            };
            sock.bind(SocketAddr::new(ip, 0))?;
            sock.connect(addr).await
        }
    }
}
