//! Serverless probes: measure a device that cannot run nettest-server (IP phone, switch,
//! gateway, printer) with a protocol it already speaks.
//!
//! Each prober answers one question per probe, "did the target respond and how fast": ICMP
//! echo (`ping`), a TCP handshake (`connect`), SIP OPTIONS over UDP (`sip`) or an HTTP HEAD
//! (`http`/`https`). They feed the same `LatencyTracker`, sinks and report as the echo modes so
//! a phone can be soaked exactly like a nettest-server.
//!
//! Invariants:
//! - `AnyProber::probe` is an owned `Send` future; the runner spawns one per sequence number so
//!   a slow target never delays the next probe.
//! - A prober never retransmits: loss is the measurement.
//! - RTT is stamped where the reply is parsed, not when the waiting task is woken.
//! - A definitive negative (refused, unreachable) is `ProbeError`, never a silent timeout.

pub mod http;
pub mod icmp;
pub mod sip;
pub mod tcp_connect;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::tls::client::TlsClientMode;
use crate::transport::{ConnectTimings, DialError, Protocol, resolve};

#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub prefer_ipv6: Option<bool>,
    pub bind: Option<IpAddr>,
    /// Per-probe deadline (also the DNS timeout). The runner passes its loss timeout so the
    /// probe future and the tracker expire together.
    pub timeout: Duration,
    /// Ping: ICMP message size (header included). Ignored by the other probers.
    pub payload_bytes: usize,
    /// http/https request path.
    pub path: String,
    /// https certificate policy. `None` = accept any certificate (there is no CA store in the
    /// zero-dependency build and devices almost always present self-signed certificates).
    pub tls: Option<TlsClientMode>,
}

impl Default for ProbeTarget {
    fn default() -> Self {
        Self {
            protocol: Protocol::Ping,
            host: "127.0.0.1".into(),
            port: 0,
            prefer_ipv6: None,
            bind: None,
            timeout: Duration::from_secs(2),
            payload_bytes: 64,
            path: "/".into(),
            tls: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("timed out")]
    Timeout,
    #[error("connection refused")]
    Refused,
    #[error("unreachable: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Io(std::io::Error),
    /// The target answered, but not in the protocol we expected.
    #[error("{0}")]
    Protocol(String),
    /// This probe cannot run here (permissions, platform, wrong protocol).
    #[error("{0}")]
    Unsupported(String),
    #[error(transparent)]
    Resolve(#[from] DialError),
}

/// A positive answer. `detail` carries the SIP or HTTP status line for the log.
#[derive(Debug, Clone)]
pub struct ProbeReply {
    pub rtt: Duration,
    pub detail: Option<String>,
}

/// One enum, not a trait object, per the transport convention. Variants are `Arc` so the
/// runner can clone one handle per spawned probe.
#[derive(Clone)]
pub enum AnyProber {
    Icmp(Arc<icmp::IcmpProber>),
    TcpConnect(Arc<tcp_connect::TcpConnectProber>),
    Sip(Arc<sip::SipProber>),
    Http(Arc<http::HttpProber>),
}

impl AnyProber {
    /// Resolve the target once and open any long-lived socket. The returned timings carry the
    /// DNS phase so the report's connect table works for serverless runs too.
    pub async fn open(t: &ProbeTarget) -> Result<(AnyProber, ConnectTimings), ProbeError> {
        let start = Instant::now();
        let port = if t.protocol == Protocol::Ping {
            0
        } else {
            t.port
        };
        let (addr, dns) = resolve(&t.host, port, t.prefer_ipv6, t.timeout).await?;
        let prober = match t.protocol {
            Protocol::Ping => {
                AnyProber::Icmp(Arc::new(icmp::IcmpProber::open(addr.ip(), t).await?))
            }
            Protocol::Connect => {
                AnyProber::TcpConnect(Arc::new(tcp_connect::TcpConnectProber::new(addr, t)))
            }
            Protocol::Sip => AnyProber::Sip(Arc::new(sip::SipProber::open(addr, t).await?)),
            Protocol::Http | Protocol::Https => {
                AnyProber::Http(Arc::new(http::HttpProber::new(addr, t)?))
            }
            other => {
                return Err(ProbeError::Unsupported(format!(
                    "{other} needs a nettest-server; serverless probes are ping, connect, sip, http, https"
                )));
            }
        };
        let timings = ConnectTimings {
            protocol: Some(t.protocol),
            peer: Some(addr),
            dns,
            total: start.elapsed(),
            ..Default::default()
        };
        Ok((prober, timings))
    }

    pub async fn probe(self, seq: u64) -> Result<ProbeReply, ProbeError> {
        match self {
            AnyProber::Icmp(p) => p.probe(seq).await,
            AnyProber::TcpConnect(p) => p.probe().await,
            AnyProber::Sip(p) => p.probe(seq).await,
            AnyProber::Http(p) => p.probe().await,
        }
    }

    /// Short description for the log, e.g. "icmp echo (56 data bytes)".
    pub fn describe(&self) -> String {
        match self {
            AnyProber::Icmp(p) => p.describe(),
            AnyProber::TcpConnect(p) => p.describe(),
            AnyProber::Sip(p) => p.describe(),
            AnyProber::Http(p) => p.describe(),
        }
    }
}

/// Reply delivered by a prober's receive task: when it was parsed, plus any status text.
pub(crate) type Answer = Result<(Instant, Option<String>), ProbeError>;

/// Outstanding probes for probers that share one socket (ICMP, SIP). The receive task completes
/// entries by sequence number; the probe future waits on its oneshot.
#[derive(Default)]
pub(crate) struct Pending {
    map: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
}

impl Pending {
    pub(crate) fn register(&self, seq: u64) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        self.map.lock().expect("pending map").insert(seq, tx);
        rx
    }

    pub(crate) fn complete(&self, seq: u64, now: Instant, detail: Option<String>) -> bool {
        match self.map.lock().expect("pending map").remove(&seq) {
            Some(tx) => tx.send(Ok((now, detail))).is_ok(),
            None => false,
        }
    }

    /// An error on the shared socket cannot be attributed to one probe: fail them all.
    pub(crate) fn fail_all(&self, make: impl Fn() -> ProbeError) {
        let drained: Vec<_> = self.map.lock().expect("pending map").drain().collect();
        for (_, tx) in drained {
            let _ = tx.send(Err(make()));
        }
    }

    pub(crate) fn forget(&self, seq: u64) {
        self.map.lock().expect("pending map").remove(&seq);
    }
}

/// Wait for the receive task's answer or the deadline.
pub(crate) async fn await_answer(
    rx: oneshot::Receiver<Answer>,
    timeout: Duration,
    pending: &Pending,
    seq: u64,
    sent_at: Instant,
) -> Result<ProbeReply, ProbeError> {
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(Ok((at, detail)))) => Ok(ProbeReply {
            rtt: at.saturating_duration_since(sent_at),
            detail,
        }),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err(ProbeError::Io(std::io::Error::other(
            "probe receive task stopped",
        ))),
        Err(_) => {
            pending.forget(seq);
            Err(ProbeError::Timeout)
        }
    }
}

/// Classify a socket error into the probe outcome vocabulary.
pub(crate) fn map_io(e: std::io::Error) -> ProbeError {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => ProbeError::Refused,
        K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown => {
            ProbeError::Unreachable(e.to_string())
        }
        K::TimedOut => ProbeError::Timeout,
        _ => ProbeError::Io(e),
    }
}

/// Local address to bind for `addr`'s family: the requested source IP or the wildcard.
pub(crate) fn bind_for(addr: &SocketAddr, bind: Option<IpAddr>) -> SocketAddr {
    match bind {
        Some(ip) => SocketAddr::new(ip, 0),
        None if addr.is_ipv6() => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
        None => SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
    }
}

/// Host as it appears in a URI or Host header: IPv6 literals get brackets.
pub(crate) fn host_literal(host: &str) -> String {
    let bare = host.trim_matches(['[', ']']);
    if bare.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{bare}]")
    } else {
        bare.to_string()
    }
}
