//! Identification for anything that is not a nettest client.
//!
//! Security scanners must be able to identify what listens on a port, so every listener answers
//! non-nettest traffic with the configured banner instead of silence. Stream listeners peek at
//! the first bytes before handing the connection to the protocol code: nettest frames start
//! with the magic, a WebSocket upgrade is an HTTP request with `Upgrade: websocket`, and
//! everything else (other HTTP, random bytes, or nothing at all within `BANNER_WAIT`) gets the
//! banner and a close. UDP answers junk datagrams the same way, rate-limited so the server can
//! never be used to amplify traffic towards a spoofed source.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use nettest_proto::config::ServerConfig;
use nettest_proto::frame::MAGIC;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// nmap's null probe waits 6 s for a banner; a real client sends Hello right after connecting.
pub const BANNER_WAIT: Duration = Duration::from_secs(3);
const MAX_HEAD: usize = 8 * 1024;

pub enum FirstBytes {
    /// Starts with the nettest magic; the bytes must be replayed to the frame decoder.
    Nettest(Vec<u8>),
    /// A complete HTTP request head asking for a WebSocket upgrade.
    WsUpgrade(Vec<u8>),
    /// Anything else, including nothing at all.
    Other(Vec<u8>),
}

impl FirstBytes {
    pub fn bytes(&self) -> &[u8] {
        match self {
            FirstBytes::Nettest(b) | FirstBytes::WsUpgrade(b) | FirstBytes::Other(b) => b,
        }
    }
}

/// Read just enough to classify the connection, or give up at the deadline.
pub async fn classify<S: AsyncRead + Unpin>(
    s: &mut S,
    wait: Duration,
) -> std::io::Result<FirstBytes> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    let mut tmp = [0u8; 1024];
    loop {
        if let Some(v) = decide(&buf) {
            return Ok(v);
        }
        if buf.len() >= MAX_HEAD {
            return Ok(FirstBytes::Other(buf));
        }
        match tokio::time::timeout_at(deadline, s.read(&mut tmp)).await {
            Ok(Ok(0)) => return Ok(FirstBytes::Other(buf)),
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(FirstBytes::Other(buf)),
        }
    }
}

/// `None` means "need more bytes".
pub fn decide(buf: &[u8]) -> Option<FirstBytes> {
    if buf.is_empty() {
        return None;
    }
    if buf.len() < MAGIC.len() {
        if MAGIC.starts_with(buf) || http_request_line(buf).is_none() {
            return None;
        }
        return Some(FirstBytes::Other(buf.to_vec()));
    }
    if buf.starts_with(&MAGIC) {
        return Some(FirstBytes::Nettest(buf.to_vec()));
    }
    match http_request_line(buf) {
        Some(true) => {
            let end = head_end(buf)?;
            Some(if wants_websocket(&buf[..end]) {
                FirstBytes::WsUpgrade(buf.to_vec())
            } else {
                FirstBytes::Other(buf.to_vec())
            })
        }
        Some(false) => Some(FirstBytes::Other(buf.to_vec())),
        None => None,
    }
}

/// `Some(true)`: starts with an HTTP method token and a space. `Some(false)`: definitely not
/// HTTP. `None`: only uppercase letters so far, undecided.
fn http_request_line(buf: &[u8]) -> Option<bool> {
    for (i, b) in buf.iter().enumerate().take(16) {
        if *b == b' ' {
            return Some(i > 0);
        }
        if !b.is_ascii_uppercase() {
            return Some(false);
        }
    }
    (buf.len() >= 16).then_some(false)
}

/// True when the bytes begin with an HTTP request line (used to pick the reply format).
pub fn looks_like_http(buf: &[u8]) -> bool {
    http_request_line(buf) == Some(true)
}

fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
}

fn wants_websocket(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    text.lines().skip(1).any(|l| {
        l.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("upgrade")
                && value.to_ascii_lowercase().contains("websocket")
        })
    })
}

/// The banner, as an HTTP response when the peer spoke HTTP, else as a bare line.
pub fn banner_message(cfg: &ServerConfig, http: bool) -> String {
    let body = cfg.banner_line();
    if http {
        format!(
            "HTTP/1.1 200 OK\r\nServer: nettest/{}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            env!("CARGO_PKG_VERSION"),
            body.len()
        )
    } else {
        body
    }
}

pub async fn reply_banner<S: AsyncWrite + Unpin>(
    s: &mut S,
    cfg: &ServerConfig,
    http: bool,
) -> std::io::Result<()> {
    s.write_all(banner_message(cfg, http).as_bytes()).await?;
    s.flush().await?;
    s.shutdown().await
}

/// UDP banner rate limit: one reply per source address per 2 s and 50 per second overall. The
/// banner is ~60 bytes, so even a flood of 1-byte probes cannot reflect more than ~3 KB/s.
pub struct BannerLimiter {
    per_peer: HashMap<IpAddr, Instant>,
    window_start: Instant,
    window_count: u32,
}

impl BannerLimiter {
    const PER_PEER: Duration = Duration::from_secs(2);
    const PER_SECOND: u32 = 50;

    pub fn new(now: Instant) -> Self {
        Self {
            per_peer: HashMap::new(),
            window_start: now,
            window_count: 0,
        }
    }

    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.window_count = 0;
        }
        if self.window_count >= Self::PER_SECOND {
            return false;
        }
        if let Some(last) = self.per_peer.get(&ip)
            && now.duration_since(*last) < Self::PER_PEER
        {
            return false;
        }
        self.per_peer.insert(ip, now);
        self.window_count += 1;
        true
    }

    pub fn sweep(&mut self, now: Instant) {
        self.per_peer
            .retain(|_, last| now.duration_since(*last) < Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(decide(b"").is_none());
        assert!(decide(b"NT").is_none());
        assert!(matches!(
            decide(b"NTP1\x01\x02"),
            Some(FirstBytes::Nettest(_))
        ));
        assert!(decide(b"GET / HTTP/1.1\r\nHost: x\r\n").is_none());
        assert!(matches!(
            decide(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(FirstBytes::Other(_))
        ));
        assert!(matches!(
            decide(
                b"GET /ws HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUPGRADE: WebSocket\r\n\r\n"
            ),
            Some(FirstBytes::WsUpgrade(_))
        ));
        assert!(matches!(
            decide(b"\x16\x03\x01\x00"),
            Some(FirstBytes::Other(_))
        ));
        assert!(matches!(decide(b"\x00"), Some(FirstBytes::Other(_))));
        assert!(matches!(
            decide(b"ABCDEFGHIJKLMNOPQ"),
            Some(FirstBytes::Other(_))
        ));
        assert!(looks_like_http(b"HEAD / HTTP/1.0\r\n\r\n"));
        assert!(!looks_like_http(b"NTP1"));
    }

    #[test]
    fn banner_formats() {
        let cfg = ServerConfig::default();
        let http = banner_message(&cfg, true);
        assert!(http.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(http.contains("Slaytons Technology Services"));
        assert!(banner_message(&cfg, false).ends_with('\n'));
    }

    #[test]
    fn limiter_caps() {
        let t0 = Instant::now();
        let mut l = BannerLimiter::new(t0);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(l.allow(ip, t0));
        assert!(!l.allow(ip, t0 + Duration::from_millis(500)));
        assert!(l.allow(ip, t0 + Duration::from_millis(2500)));
        let mut l = BannerLimiter::new(t0);
        let allowed = (0..200u32)
            .filter(|i| l.allow(format!("10.1.{}.{}", i / 250, i % 250).parse().unwrap(), t0))
            .count();
        assert_eq!(allowed, 50);
        assert!(l.allow("10.9.9.9".parse().unwrap(), t0 + Duration::from_secs(1)));
    }
}
