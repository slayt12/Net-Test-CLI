//! Raw TCP transport: `Framed<TcpStream, NtCodec>`.

use std::net::SocketAddr;

use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, FramedParts};

use super::TransportError;
use crate::frame::{Frame, NtCodec};

pub struct TcpTransport {
    inner: Framed<TcpStream, NtCodec>,
    peer: Option<SocketAddr>,
}

impl TcpTransport {
    pub fn new(stream: TcpStream) -> Self {
        let _ = stream.set_nodelay(true);
        let peer = stream.peer_addr().ok();
        Self {
            inner: Framed::new(stream, NtCodec),
            peer,
        }
    }

    /// Like `new`, but `prefix` (bytes the server already read while classifying the
    /// connection) is decoded before anything else from the socket.
    pub fn with_prefix(stream: TcpStream, prefix: &[u8]) -> Self {
        let _ = stream.set_nodelay(true);
        let peer = stream.peer_addr().ok();
        let mut parts = FramedParts::new::<Frame>(stream, NtCodec);
        parts.read_buf = BytesMut::from(prefix);
        Self {
            inner: Framed::from_parts(parts),
            peer,
        }
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        self.inner.send(frame).await.map_err(Into::into)
    }

    pub async fn recv(&mut self) -> Result<Frame, TransportError> {
        match self.inner.next().await {
            Some(Ok(f)) => Ok(f),
            Some(Err(e)) => Err(e.into()),
            None => Err(TransportError::Closed),
        }
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        self.inner.close().await.map_err(Into::into)
    }
}
