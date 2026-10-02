//! UDP transport: one datagram per frame over a *connected* socket.
//!
//! Connected sockets let the kernel filter stray datagrams and surface ICMP unreachable errors
//! on the next recv, which is useful diagnostic information in its own right.

use std::net::SocketAddr;

use tokio::net::UdpSocket;

use super::TransportError;
use crate::frame::{Frame, HEADER_LEN};

/// Default maximum datagram we are willing to send without the user opting in: fits inside a
/// 1500-byte Ethernet MTU with IPv6 + UDP headers to spare.
pub const DEFAULT_MAX_DATAGRAM: usize = 1400;
pub const MAX_DATAGRAM: usize = 65_507;

pub struct UdpTransport {
    sock: UdpSocket,
    peer: Option<SocketAddr>,
    buf: Vec<u8>,
}

impl UdpTransport {
    pub fn new(sock: UdpSocket) -> Self {
        let peer = sock.peer_addr().ok();
        Self {
            sock,
            peer,
            buf: vec![0u8; MAX_DATAGRAM + HEADER_LEN],
        }
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub async fn send(&mut self, frame: Frame) -> Result<(), TransportError> {
        let bytes = frame.to_bytes();
        if bytes.len() > MAX_DATAGRAM {
            return Err(crate::frame::FrameError::TooLarge(bytes.len() as u32).into());
        }
        self.sock.send(&bytes).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Frame, TransportError> {
        loop {
            let n = self.sock.recv(&mut self.buf).await?;
            // A datagram that is not ours (wrong magic) is ignored rather than fatal: UDP ports
            // get scanned, and one junk packet must not end a multi-hour soak test.
            match Frame::parse(&self.buf[..n]) {
                Ok(f) => return Ok(f),
                Err(crate::frame::FrameError::BadMagic) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}
