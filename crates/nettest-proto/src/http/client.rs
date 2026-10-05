//! `post()`: resolve, connect (optionally TLS), write one request, read one response.

use std::net::IpAddr;
use std::time::Duration;

use rustls_pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Url;
use crate::probe::http::parse_status_code;
use crate::tls::client::{TlsClientMode, client_config};
use crate::transport::ws::MaybeTls;
use crate::transport::{DialError, connect_tcp, resolve};

const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub timeout: Duration,
    pub tls: TlsClientMode,
    pub prefer_ipv6: Option<bool>,
    pub bind: Option<IpAddr>,
    /// Response bodies beyond this are truncated (the status is what matters).
    pub max_body: usize,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            tls: TlsClientMode::WebPki,
            prefer_ipv6: None,
            bind: None,
            max_body: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)]
    Dial(#[from] DialError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(String),
    #[error("timed out")]
    Timeout,
    #[error("{0}")]
    Malformed(String),
}

/// One `POST`. `headers` are extra request headers (validated); the body is sent verbatim.
pub async fn post(
    url: &Url,
    headers: &[(&str, &str)],
    content_type: &str,
    body: &[u8],
    opts: &HttpOptions,
) -> Result<Response, HttpError> {
    let request = build_request(url, headers, content_type, body.len())?;
    let fut = async {
        let (addr, _dns) = resolve(&url.host, url.port, opts.prefer_ipv6, opts.timeout).await?;
        let stream = connect_tcp(addr, opts.bind).await?;
        let _ = stream.set_nodelay(true);
        let mut stream = if url.https {
            let cfg = client_config(opts.tls).map_err(|e| HttpError::Tls(e.to_string()))?;
            let name = ServerName::try_from(url.host.clone())
                .map_err(|_| HttpError::Malformed(format!("invalid server name '{}'", url.host)))?;
            let tls = tokio_rustls::TlsConnector::from(cfg)
                .connect(name, stream)
                .await
                .map_err(|e| HttpError::Tls(e.to_string()))?;
            MaybeTls::ClientTls(Box::new(tls))
        } else {
            MaybeTls::Plain(stream)
        };
        stream.write_all(request.as_bytes()).await?;
        stream.write_all(body).await?;
        stream.flush().await?;
        read_response(&mut stream, opts.max_body).await
    };
    match tokio::time::timeout(opts.timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(HttpError::Timeout),
    }
}

fn build_request(
    url: &Url,
    headers: &[(&str, &str)],
    content_type: &str,
    len: usize,
) -> Result<String, HttpError> {
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: nettest/{}\r\nAccept: */*\r\nConnection: close\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        url.path,
        url.host_header(),
        env!("CARGO_PKG_VERSION"),
        content_type,
        len
    );
    for (k, v) in headers {
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(HttpError::Malformed(format!("bad header name '{k}'")));
        }
        if v.bytes().any(|b| b == b'\r' || b == b'\n' || !(b == b'\t' || (0x20..0x7f).contains(&b))) {
            return Err(HttpError::Malformed(format!(
                "header '{k}' must be printable ASCII without line breaks"
            )));
        }
        req.push_str(k);
        req.push_str(": ");
        req.push_str(v);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    Ok(req)
}

async fn read_response<S: tokio::io::AsyncRead + Unpin>(
    s: &mut S,
    max_body: usize,
) -> Result<Response, HttpError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut tmp = [0u8; 2048];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(HttpError::Malformed("response headers exceed 16 KiB".into()));
        }
        let n = s.read(&mut tmp).await?;
        if n == 0 {
            return Err(HttpError::Malformed(
                "connection closed before the response headers ended".into(),
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default().trim();
    let (status, reason) = parse_status_code(status_line)
        .ok_or_else(|| HttpError::Malformed(format!("not HTTP: {status_line:.60}")))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok());

    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    let want = content_length.unwrap_or(usize::MAX).min(max_body);
    while body.len() < want {
        let n = s.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(want);
    Ok(Response {
        status,
        reason,
        headers,
        body,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn fake_server(reply: &'static str, delay: Duration) -> (Url, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = s.read(&mut tmp).await.unwrap();
                got.extend_from_slice(&tmp[..n]);
                if let Some(i) = find(&got, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&got[..i]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .map(|v| v.parse().unwrap())
                        .unwrap_or(0);
                    if got.len() >= i + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            tokio::time::sleep(delay).await;
            s.write_all(reply.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (Url::parse(&format!("http://127.0.0.1:{port}/hook?x=1")).unwrap(), h)
    }

    #[tokio::test]
    async fn post_with_content_length_reply() {
        let (url, server) =
            fake_server("HTTP/1.1 204 No Content\r\nX-Test: 1\r\nContent-Length: 0\r\n\r\n", Duration::ZERO).await;
        let opts = HttpOptions { timeout: Duration::from_secs(5), ..Default::default() };
        let r = post(&url, &[("Title", "hi"), ("Priority", "high")], "text/plain", b"body!", &opts)
            .await
            .unwrap();
        assert_eq!((r.status, r.reason.as_str()), (204, "No Content"));
        assert!(r.is_success());
        assert_eq!(r.header("x-test"), Some("1"));
        assert!(r.body.is_empty());
        let req = server.await.unwrap();
        assert!(req.starts_with("POST /hook?x=1 HTTP/1.1\r\n"), "{req}");
        assert!(req.contains(&format!("Host: 127.0.0.1:{}\r\n", url.port)));
        assert!(req.contains("Content-Length: 5\r\n"));
        assert!(req.contains("Title: hi\r\n"));
        assert!(req.contains("Connection: close\r\n"));
        assert!(req.ends_with("\r\n\r\nbody!"));
    }

    #[tokio::test]
    async fn post_eof_delimited_body() {
        let (url, _server) =
            fake_server("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"id\":\"1\"}", Duration::ZERO).await;
        let opts = HttpOptions { timeout: Duration::from_secs(5), ..Default::default() };
        let r = post(&url, &[], "application/json", b"{}", &opts).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body_text(), "{\"id\":\"1\"}");
    }

    #[tokio::test]
    async fn post_times_out() {
        let (url, _server) = fake_server("HTTP/1.1 200 OK\r\n\r\n", Duration::from_secs(5)).await;
        let opts = HttpOptions { timeout: Duration::from_millis(300), ..Default::default() };
        assert!(matches!(post(&url, &[], "text/plain", b"x", &opts).await, Err(HttpError::Timeout)));
    }

    #[test]
    fn header_injection_rejected() {
        let url = Url::parse("http://h/x").unwrap();
        assert!(build_request(&url, &[("Title", "a\r\nX: y")], "text/plain", 0).is_err());
        assert!(build_request(&url, &[("Bad Name", "a")], "text/plain", 0).is_err());
        assert!(build_request(&url, &[("Title", "ok")], "text/plain", 0).is_ok());
    }
}
