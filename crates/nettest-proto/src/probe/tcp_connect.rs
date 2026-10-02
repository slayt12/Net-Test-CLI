//! `connect://host:port`: time the TCP three-way handshake, then close.
//!
//! Works against any open port without privileges and distinguishes "refused" (host up, port
//! closed) from "unreachable" and "timed out", which is exactly what a phone-is-down ticket
//! needs. The connection is closed with a normal FIN so devices do not log aborted sessions.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use super::{ProbeError, ProbeReply, ProbeTarget, map_io};
use crate::transport::connect_tcp;

pub struct TcpConnectProber {
    addr: SocketAddr,
    bind: Option<IpAddr>,
    timeout: Duration,
}

impl TcpConnectProber {
    pub fn new(addr: SocketAddr, t: &ProbeTarget) -> Self {
        Self {
            addr,
            bind: t.bind,
            timeout: t.timeout,
        }
    }

    pub async fn probe(&self) -> Result<ProbeReply, ProbeError> {
        let t0 = Instant::now();
        match tokio::time::timeout(self.timeout, connect_tcp(self.addr, self.bind)).await {
            Ok(Ok(stream)) => {
                let rtt = t0.elapsed();
                drop(stream);
                Ok(ProbeReply { rtt, detail: None })
            }
            Ok(Err(e)) => Err(map_io(e)),
            Err(_) => Err(ProbeError::Timeout),
        }
    }

    pub fn describe(&self) -> String {
        format!("tcp connect to {}", self.addr)
    }
}
