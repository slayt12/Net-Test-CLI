//! `sip://host[:port]`: SIP OPTIONS over UDP, the standard keep-alive IP phones and PBXs answer.
//!
//! Any final response (200, 401, 405, 486...) proves the SIP stack is up; the status line goes
//! into the sample detail. Responses are matched by CSeq and must carry this run's id in the
//! Call-ID or Via branch so a phone replaying an old transaction cannot be mistaken for a reply.
//! One socket is shared by all probes; a receive task completes them by sequence number.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use super::{
    Pending, ProbeError, ProbeReply, ProbeTarget, await_answer, bind_for, host_literal, map_io,
};

pub struct SipProber {
    sock: Arc<UdpSocket>,
    pending: Arc<Pending>,
    host: String,
    port: u16,
    local: SocketAddr,
    run_id: String,
    timeout: Duration,
    task: JoinHandle<()>,
}

impl SipProber {
    pub async fn open(addr: SocketAddr, t: &ProbeTarget) -> Result<Self, ProbeError> {
        let sock = UdpSocket::bind(bind_for(&addr, t.bind))
            .await
            .map_err(ProbeError::Io)?;
        sock.connect(addr).await.map_err(map_io)?;
        let local = sock.local_addr().map_err(ProbeError::Io)?;
        let sock = Arc::new(sock);
        let pending = Arc::new(Pending::default());
        let run_id = format!(
            "{:08x}",
            (crate::time::now_ns() as u32) ^ std::process::id().rotate_left(16)
        );
        let task = tokio::spawn(recv_loop(sock.clone(), pending.clone(), run_id.clone()));
        Ok(Self {
            sock,
            pending,
            host: host_literal(&t.host),
            port: addr.port(),
            local,
            run_id,
            timeout: t.timeout,
            task,
        })
    }

    pub async fn probe(&self, seq: u64) -> Result<ProbeReply, ProbeError> {
        let rx = self.pending.register(seq);
        let req = build_request(&self.host, self.port, self.local, &self.run_id, seq);
        let t0 = Instant::now();
        if let Err(e) = self.sock.send(req.as_bytes()).await {
            self.pending.forget(seq);
            return Err(map_io(e));
        }
        await_answer(rx, self.timeout, &self.pending, seq, t0).await
    }

    pub fn describe(&self) -> String {
        format!("SIP OPTIONS to sip:{}:{} over udp", self.host, self.port)
    }
}

impl Drop for SipProber {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn recv_loop(sock: Arc<UdpSocket>, pending: Arc<Pending>, run_id: String) {
    let mut buf = vec![0u8; 4096];
    loop {
        match sock.recv(&mut buf).await {
            Ok(n) => {
                let now = Instant::now();
                if let Some(r) = parse_response(&buf[..n])
                    && r.is_ours(&run_id)
                    && r.code >= 200
                    && let Some(seq) = r.cseq
                {
                    pending.complete(seq, now, Some(format!("SIP/2.0 {} {}", r.code, r.reason)));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                // ICMP port unreachable surfaced on the connected socket: nothing listens there.
                pending.fail_all(|| ProbeError::Unreachable("udp port unreachable".into()));
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// Minimal RFC 3261 OPTIONS request. `local` is the connected socket's address so Via/Contact
/// are routable even on multi-homed hosts.
pub fn build_request(host: &str, port: u16, local: SocketAddr, run_id: &str, seq: u64) -> String {
    let lip = host_literal(&local.ip().to_string());
    let lport = local.port();
    format!(
        "OPTIONS sip:{host}:{port} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {lip}:{lport};branch=z9hG4bK-nettest-{run_id}-{seq};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:nettest@{lip}>;tag={run_id}\r\n\
         To: <sip:{host}:{port}>\r\n\
         Call-ID: {run_id}-{seq}@{lip}\r\n\
         CSeq: {seq} OPTIONS\r\n\
         Contact: <sip:nettest@{lip}:{lport}>\r\n\
         Accept: application/sdp\r\n\
         User-Agent: nettest/{}\r\n\
         Content-Length: 0\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipResponse {
    pub code: u16,
    pub reason: String,
    pub cseq: Option<u64>,
    pub call_id: Option<String>,
    pub branch: Option<String>,
}

impl SipResponse {
    pub fn is_ours(&self, run_id: &str) -> bool {
        self.call_id.as_deref().is_some_and(|c| c.contains(run_id))
            || self.branch.as_deref().is_some_and(|b| b.contains(run_id))
    }
}

/// Parse a SIP response head. Tolerates LF-only line endings, folded headers and compact
/// header names. Returns `None` for anything that is not a `SIP/2.0` status line.
pub fn parse_response(buf: &[u8]) -> Option<SipResponse> {
    let text = String::from_utf8_lossy(buf);
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            let last = lines.last_mut().expect("non-empty");
            last.push(' ');
            last.push_str(line.trim());
        } else {
            lines.push(line.to_string());
        }
    }
    let status = lines.first()?;
    let rest = status.strip_prefix("SIP/2.0 ")?;
    let (code, reason) = rest.split_once(' ').unwrap_or((rest, ""));
    let code: u16 = code.parse().ok()?;
    let mut r = SipResponse {
        code,
        reason: reason.trim().to_string(),
        cseq: None,
        call_id: None,
        branch: None,
    };
    for h in &lines[1..] {
        let Some((name, value)) = h.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "cseq" => r.cseq = value.split_whitespace().next().and_then(|n| n.parse().ok()),
            "call-id" | "i" => r.call_id = Some(value.to_string()),
            "via" | "v" => {
                if let Some(pos) = value.find("branch=") {
                    let b = &value[pos + 7..];
                    let end = b.find([';', ',', ' ', '\t']).unwrap_or(b.len());
                    r.branch = Some(b[..end].to_string());
                }
            }
            _ => {}
        }
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_required_headers() {
        let req = build_request(
            "10.0.0.7",
            5060,
            "192.168.1.5:40000".parse().unwrap(),
            "abcd1234",
            7,
        );
        assert!(req.starts_with("OPTIONS sip:10.0.0.7:5060 SIP/2.0\r\n"));
        assert!(req.contains("CSeq: 7 OPTIONS\r\n"));
        assert!(req.contains("Call-ID: abcd1234-7@192.168.1.5\r\n"));
        assert!(req.contains("branch=z9hG4bK-nettest-abcd1234-7;rport"));
        assert!(req.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn parses_folded_and_compact_headers() {
        let raw = "SIP/2.0 200 OK\r\nVIA: SIP/2.0/UDP 1.2.3.4:5060;branch=z9hG4bK-nettest-run1-3;rport=5060\r\n i: run1-3@1.2.3.4\r\nCSeq: 3\r\n OPTIONS\r\nContent-Length: 0\r\n\r\n";
        let r = parse_response(raw.as_bytes()).unwrap();
        assert_eq!(r.code, 200);
        assert_eq!(r.reason, "OK");
        assert_eq!(r.cseq, Some(3));
        assert!(r.is_ours("run1"));
        assert!(!r.is_ours("other"));
        assert_eq!(r.branch.as_deref(), Some("z9hG4bK-nettest-run1-3"));
    }

    #[test]
    fn rejects_non_sip() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n\r\n").is_none());
        assert!(parse_response(b"\x00\x01\x02").is_none());
        let r = parse_response(b"SIP/2.0 100 Trying\nCSeq: 9 OPTIONS\nCall-ID: x\n\n").unwrap();
        assert_eq!((r.code, r.cseq), (100, Some(9)));
    }
}
