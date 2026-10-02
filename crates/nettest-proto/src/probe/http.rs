//! `http://` and `https://`: a HEAD request to a device's web interface, timed to the status
//! line. Any HTTP status counts as alive (a 401 from a phone's admin page still proves the
//! web stack is up); the status line is kept as the sample detail.
//!
//! https accepts any certificate unless a fingerprint is pinned: there is no CA store in the
//! zero-dependency build and devices present self-signed certificates almost without exception.
//! The probe measures reachability, it does not authenticate the device.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls_pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{ProbeError, ProbeReply, ProbeTarget, host_literal, map_io};
use crate::tls::client::{TlsClientMode, client_config};
use crate::transport::{Protocol, connect_tcp};

pub struct HttpProber {
    addr: SocketAddr,
    bind: Option<IpAddr>,
    request: String,
    tls: Option<(Arc<rustls::ClientConfig>, ServerName<'static>)>,
    timeout: Duration,
}

impl HttpProber {
    pub fn new(addr: SocketAddr, t: &ProbeTarget) -> Result<Self, ProbeError> {
        let host = host_literal(&t.host);
        let default_port = if t.protocol == Protocol::Https {
            443
        } else {
            80
        };
        let host_header = if addr.port() == default_port {
            host.clone()
        } else {
            format!("{host}:{}", addr.port())
        };
        let path = if t.path.starts_with('/') {
            t.path.clone()
        } else {
            format!("/{}", t.path)
        };
        let request = format!(
            "HEAD {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: nettest/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
            env!("CARGO_PKG_VERSION")
        );
        let tls = if t.protocol == Protocol::Https {
            let cfg = client_config(t.tls.unwrap_or(TlsClientMode::Insecure))
                .map_err(|e| ProbeError::Protocol(format!("tls config: {e}")))?;
            let name = ServerName::try_from(t.host.trim_matches(['[', ']']).to_string())
                .map_err(|_| ProbeError::Protocol(format!("invalid server name '{}'", t.host)))?;
            Some((cfg, name))
        } else {
            None
        };
        Ok(Self {
            addr,
            bind: t.bind,
            request,
            tls,
            timeout: t.timeout,
        })
    }

    pub async fn probe(&self) -> Result<ProbeReply, ProbeError> {
        let t0 = Instant::now();
        let fut = async {
            let stream = connect_tcp(self.addr, self.bind).await.map_err(map_io)?;
            let _ = stream.set_nodelay(true);
            match &self.tls {
                Some((cfg, name)) => {
                    let tls = tokio_rustls::TlsConnector::from(cfg.clone())
                        .connect(name.clone(), stream)
                        .await
                        .map_err(|e| ProbeError::Protocol(format!("tls handshake: {e}")))?;
                    exchange(tls, &self.request).await
                }
                None => exchange(stream, &self.request).await,
            }
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(status)) => Ok(ProbeReply {
                rtt: t0.elapsed(),
                detail: Some(status),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ProbeError::Timeout),
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} HEAD to {}",
            if self.tls.is_some() { "https" } else { "http" },
            self.addr
        )
    }
}

/// Send the request, return the status line. Reads only until the first line ends so a slow
/// body never inflates the RTT.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    request: &str,
) -> Result<String, ProbeError> {
    s.write_all(request.as_bytes())
        .await
        .map_err(ProbeError::Io)?;
    let mut buf = Vec::with_capacity(256);
    let mut tmp = [0u8; 512];
    loop {
        let n = s.read(&mut tmp).await.map_err(ProbeError::Io)?;
        if n == 0 {
            return Err(ProbeError::Protocol(
                "closed before an HTTP status line".into(),
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..pos]).trim().to_string();
            return parse_status(&line)
                .ok_or_else(|| ProbeError::Protocol(format!("not HTTP: {line:.40}")));
        }
        if buf.len() > 1024 {
            return Err(ProbeError::Protocol(
                "no HTTP status line in first 1 KiB".into(),
            ));
        }
    }
}

/// `HTTP/1.1 200 OK` -> the line; anything else -> `None`.
pub fn parse_status(line: &str) -> Option<String> {
    let rest = line.strip_prefix("HTTP/1.")?;
    let (_ver, rest) = rest.split_once(' ')?;
    let code = rest.split(' ').next()?;
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit())).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_status;

    #[test]
    fn status_lines() {
        assert_eq!(
            parse_status("HTTP/1.1 200 OK").as_deref(),
            Some("HTTP/1.1 200 OK")
        );
        assert_eq!(
            parse_status("HTTP/1.0 401").as_deref(),
            Some("HTTP/1.0 401")
        );
        assert!(parse_status("SIP/2.0 200 OK").is_none());
        assert!(parse_status("HTTP/1.1 twohundred").is_none());
    }
}
