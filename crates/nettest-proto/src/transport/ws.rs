//! WebSocket transport (ws:// and wss://), one binary message per frame.
//!
//! TLS is performed by tokio-rustls *before* the WebSocket handshake and the resulting stream is
//! handed to tungstenite. tokio-tungstenite's own TLS features are deliberately not enabled: this
//! keeps a single crypto provider in the binary and lets `dial()` time TCP, TLS and the HTTP
//! upgrade as separate phases.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use super::rewind::Rewind;
use super::{Protocol, TransportError};
use crate::frame::Frame;

/// A TCP stream that may be wrapped in TLS from either side of the connection.
pub enum MaybeTls {
    Plain(TcpStream),
    ClientTls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    ServerTls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl MaybeTls {
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        match self {
            MaybeTls::Plain(s) => s.peer_addr().ok(),
            MaybeTls::ClientTls(s) => s.get_ref().0.peer_addr().ok(),
            MaybeTls::ServerTls(s) => s.get_ref().0.peer_addr().ok(),
        }
    }

    pub fn is_tls(&self) -> bool {
        !matches!(self, MaybeTls::Plain(_))
    }
}

impl AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTls::ClientTls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            MaybeTls::ServerTls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTls::ClientTls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            MaybeTls::ServerTls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTls::ClientTls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            MaybeTls::ServerTls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTls::ClientTls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            MaybeTls::ServerTls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// The stream tungstenite drives: TLS or plain, with any bytes the server peeked replayed first.
pub type WsStream = WebSocketStream<Rewind<MaybeTls>>;

pub struct WsTransport {
    inner: WsStream,
    peer: Option<SocketAddr>,
    tls: bool,
}

impl WsTransport {
    pub fn new(inner: WsStream) -> Self {
        let peer = inner.get_ref().get_ref().peer_addr();
        let tls = inner.get_ref().get_ref().is_tls();
        Self { inner, peer, tls }
    }

    pub fn protocol(&self) -> Protocol {
        if self.tls {
            Protocol::Wss
        } else {
            Protocol::Ws
        }
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        self.inner
            .send(Message::Binary(frame.to_bytes()))
            .await
            .map_err(map_ws_err)
    }

    pub async fn recv(&mut self) -> Result<Frame, TransportError> {
        loop {
            match self.inner.next().await {
                Some(Ok(Message::Binary(b))) => return Ok(Frame::parse(&b)?),
                // Ping/Pong are answered by tungstenite internally; Text is not part of the
                // protocol and is ignored so a curious browser cannot kill a session.
                Some(Ok(Message::Close(_))) | None => return Err(TransportError::Closed),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(map_ws_err(e)),
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        match self.inner.close(None).await {
            Ok(()) => Ok(()),
            Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => Ok(()),
            Err(e) => Err(map_ws_err(e)),
        }
    }
}

fn map_ws_err(e: WsError) -> TransportError {
    match e {
        WsError::ConnectionClosed | WsError::AlreadyClosed => TransportError::Closed,
        WsError::Io(io) => TransportError::Io(io),
        other => TransportError::Ws(other),
    }
}
