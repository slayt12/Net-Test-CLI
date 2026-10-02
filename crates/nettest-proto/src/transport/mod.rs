//! Transport layer: one `AnyTransport` enum over TCP, UDP and WebSocket (plain or TLS).
//!
//! An enum rather than `Box<dyn Trait>` keeps the hot path monomorphised and avoids an
//! `async-trait` dependency. Every `recv()` is cancel-safe so callers can use it inside
//! `tokio::select!` without losing frames.
//!
//! `Protocol` also names the serverless probe methods (ping, connect, sip, http, https). Those
//! never produce an `AnyTransport`; they live in `crate::probe` and share only the name space so
//! the client has one "protocol" setting.

mod dial;
pub mod rewind;
pub mod tcp;
pub mod udp;
pub mod ws;

pub(crate) use dial::connect_tcp;
pub use dial::{DialError, DialOptions, dial, resolve};
pub use rewind::Rewind;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::frame::{Frame, FrameError, Header};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
    Ws,
    Wss,
    /// ICMP echo to any host (serverless).
    Ping,
    /// TCP connect handshake to any open port (serverless).
    Connect,
    /// SIP OPTIONS over UDP, answered by phones and PBXs (serverless).
    Sip,
    /// HTTP HEAD to a device's web interface (serverless).
    Http,
    /// HTTPS HEAD, any certificate accepted unless pinned (serverless).
    Https,
}

impl Protocol {
    /// Display and cycle order: nettest-server protocols first, then serverless probes.
    pub const ALL: [Protocol; 9] = [
        Protocol::Ws,
        Protocol::Wss,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Ping,
        Protocol::Connect,
        Protocol::Sip,
        Protocol::Http,
        Protocol::Https,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
            Protocol::Ws => "ws",
            Protocol::Wss => "wss",
            Protocol::Ping => "ping",
            Protocol::Connect => "connect",
            Protocol::Sip => "sip",
            Protocol::Http => "http",
            Protocol::Https => "https",
        }
    }

    /// True when the far end must be a nettest-server (echo protocol over this transport).
    pub fn needs_server(self) -> bool {
        matches!(
            self,
            Protocol::Tcp | Protocol::Udp | Protocol::Ws | Protocol::Wss
        )
    }

    /// True for probes that work against any device (phone, switch, gateway).
    pub fn is_serverless(self) -> bool {
        !self.needs_server()
    }

    /// Port implied by the URL scheme when the user gives none. `None` = keep what is set
    /// (ping has no port; connect must name one).
    pub fn default_port(self) -> Option<u16> {
        match self {
            Protocol::Tcp | Protocol::Udp => Some(9100),
            Protocol::Ws => Some(9101),
            Protocol::Wss => Some(9102),
            Protocol::Sip => Some(5060),
            Protocol::Http => Some(80),
            Protocol::Https => Some(443),
            Protocol::Ping | Protocol::Connect => None,
        }
    }

    /// nettest TLS (wss) only; https probes handle TLS themselves.
    pub fn is_tls(self) -> bool {
        matches!(self, Protocol::Wss)
    }

    pub fn is_websocket(self) -> bool {
        matches!(self, Protocol::Ws | Protocol::Wss)
    }

    /// Next protocol in display order (used by the TUI "cycle" key).
    pub fn next(self) -> Protocol {
        let i = Protocol::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Protocol::ALL[(i + 1) % Protocol::ALL.len()]
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Protocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let l = s.to_ascii_lowercase();
        Protocol::ALL
            .iter()
            .copied()
            .find(|p| p.as_str() == l)
            .ok_or_else(|| {
                format!("unknown protocol '{s}' (ws|wss|tcp|udp|ping|connect|sip|http|https)")
            })
    }
}

/// Per-phase connect timings. `None` means the phase did not apply to this protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectTimings {
    pub protocol: Option<Protocol>,
    pub peer: Option<SocketAddr>,
    pub dns: Option<Duration>,
    pub tcp: Option<Duration>,
    pub tls: Option<Duration>,
    pub ws_upgrade: Option<Duration>,
    pub hello: Option<Duration>,
    pub total: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connection closed by peer")]
    Closed,
    #[error("server error: {0}")]
    Remote(String),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::frame::codec_error::CodecError> for TransportError {
    fn from(e: crate::frame::codec_error::CodecError) -> Self {
        match e {
            crate::frame::codec_error::CodecError::Frame(f) => TransportError::Frame(f),
            crate::frame::codec_error::CodecError::Io(i) => TransportError::Io(i),
        }
    }
}

#[allow(clippy::large_enum_variant)] // one per connection; boxing would only add an indirection on the hot path
pub enum AnyTransport {
    Tcp(tcp::TcpTransport),
    Udp(udp::UdpTransport),
    Ws(ws::WsTransport),
}

impl AnyTransport {
    pub fn protocol(&self) -> Protocol {
        match self {
            AnyTransport::Tcp(_) => Protocol::Tcp,
            AnyTransport::Udp(_) => Protocol::Udp,
            AnyTransport::Ws(w) => w.protocol(),
        }
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match self {
            AnyTransport::Tcp(t) => t.peer_addr(),
            AnyTransport::Udp(u) => u.peer_addr(),
            AnyTransport::Ws(w) => w.peer_addr(),
        }
    }

    pub async fn send(&mut self, header: Header, payload: Bytes) -> Result<(), TransportError> {
        let frame = Frame::new(header, payload);
        match self {
            AnyTransport::Tcp(t) => t.send(frame).await,
            AnyTransport::Udp(u) => u.send(frame).await,
            AnyTransport::Ws(w) => w.send(frame).await,
        }
    }

    pub async fn send_frame(&mut self, frame: Frame) -> Result<(), TransportError> {
        match self {
            AnyTransport::Tcp(t) => t.send(frame).await,
            AnyTransport::Udp(u) => u.send(frame).await,
            AnyTransport::Ws(w) => w.send(frame).await,
        }
    }

    /// Cancel-safe.
    pub async fn recv(&mut self) -> Result<Frame, TransportError> {
        match self {
            AnyTransport::Tcp(t) => t.recv().await,
            AnyTransport::Udp(u) => u.recv().await,
            AnyTransport::Ws(w) => w.recv().await,
        }
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        match self {
            AnyTransport::Tcp(t) => t.close().await,
            AnyTransport::Udp(u) => u.close().await,
            AnyTransport::Ws(w) => w.close().await,
        }
    }
}
