//! Linux/Unix ICMP via a datagram "ping socket" (no privileges) or a raw socket (root).
//!
//! Ping sockets rewrite the identifier and deliver only replies to this socket, so matching is
//! done on the embedded 64-bit sequence. Destination-unreachable errors are not delivered
//! without `IP_RECVERR`, so an unreachable host shows as a timeout here (same as `ping`).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Socket, Type};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use super::{PERMISSION_HINT, data_len_for, packet};
use crate::probe::{Pending, ProbeError, ProbeReply, ProbeTarget, await_answer, map_io};

pub struct IcmpProber {
    sock: Arc<UdpSocket>,
    pending: Arc<Pending>,
    target: IpAddr,
    v6: bool,
    raw: bool,
    ident: u16,
    data_len: usize,
    timeout: Duration,
    task: JoinHandle<()>,
}

impl IcmpProber {
    pub async fn open(target: IpAddr, t: &ProbeTarget) -> Result<Self, ProbeError> {
        let v6 = target.is_ipv6();
        let domain = if v6 { Domain::IPV6 } else { Domain::IPV4 };
        let proto = if v6 {
            socket2::Protocol::ICMPV6
        } else {
            socket2::Protocol::ICMPV4
        };
        let (sock, raw) = match Socket::new(domain, Type::DGRAM, Some(proto)) {
            Ok(s) => (s, false),
            Err(first) => match Socket::new(domain, Type::RAW, Some(proto)) {
                Ok(s) => (s, true),
                Err(_) if first.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(ProbeError::Unsupported(PERMISSION_HINT.into()));
                }
                Err(_) => return Err(ProbeError::Io(first)),
            },
        };
        if let Some(ip) = t.bind {
            sock.bind(&SocketAddr::new(ip, 0).into())
                .map_err(ProbeError::Io)?;
        }
        sock.set_nonblocking(true).map_err(ProbeError::Io)?;
        sock.connect(&SocketAddr::new(target, 0).into())
            .map_err(map_io)?;
        let std_sock: std::net::UdpSocket = sock.into();
        let sock = Arc::new(UdpSocket::from_std(std_sock).map_err(ProbeError::Io)?);
        let pending = Arc::new(Pending::default());
        let ident = (std::process::id() & 0xffff) as u16;
        let task = tokio::spawn(recv_loop(sock.clone(), pending.clone(), v6, raw, ident));
        Ok(Self {
            sock,
            pending,
            target,
            v6,
            raw,
            ident,
            data_len: data_len_for(t.payload_bytes),
            timeout: t.timeout,
            task,
        })
    }

    pub async fn probe(&self, seq: u64) -> Result<ProbeReply, ProbeError> {
        let rx = self.pending.register(seq);
        let pkt = packet::echo_request(
            self.v6,
            self.ident,
            seq,
            crate::time::now_ns(),
            self.data_len,
        );
        let t0 = Instant::now();
        if let Err(e) = self.sock.send(&pkt).await {
            self.pending.forget(seq);
            return Err(map_io(e));
        }
        await_answer(rx, self.timeout, &self.pending, seq, t0).await
    }

    pub fn describe(&self) -> String {
        format!(
            "icmp echo to {} ({} data bytes, {} socket)",
            self.target,
            self.data_len,
            if self.raw { "raw" } else { "ping" }
        )
    }
}

impl Drop for IcmpProber {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn recv_loop(sock: Arc<UdpSocket>, pending: Arc<Pending>, v6: bool, raw: bool, ident: u16) {
    let mut buf = vec![0u8; 65536];
    loop {
        match sock.recv(&mut buf).await {
            Ok(n) => {
                let now = Instant::now();
                if let Some(r) = packet::parse_reply(&buf[..n], v6, raw && !v6)
                    // Raw sockets see every ICMP message on the host; keep only ours.
                    && (!raw || r.ident == ident)
                {
                    pending.complete(r.seq, now, None);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                pending.fail_all(|| ProbeError::Unreachable("destination unreachable".into()));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}
