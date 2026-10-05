//! Just enough URL parsing for webhook endpoints: `http[s]://host[:port][/path[?query]]`.
//! No external crate: the grammar we accept is tiny and a full parser would be the only
//! dependency added for it.

use std::fmt;

use crate::probe::host_literal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Request target including any query, always starting with `/`.
    pub path: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlError {
    #[error("url must start with http:// or https://")]
    Scheme,
    #[error("url has no host")]
    NoHost,
    #[error("bad port in url")]
    Port,
    #[error("user:password@ in a url is not supported; use the notifier's token field")]
    UserInfo,
    #[error("bad IPv6 literal in url")]
    Ipv6,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, UrlError> {
        let s = s.trim();
        let (https, rest) = if let Some(r) = strip_scheme(s, "https://") {
            (true, r)
        } else if let Some(r) = strip_scheme(s, "http://") {
            (false, r)
        } else {
            return Err(UrlError::Scheme);
        };
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(UrlError::UserInfo);
        }
        let (host, port) = if let Some(r) = authority.strip_prefix('[') {
            let (h, p) = r.split_once(']').ok_or(UrlError::Ipv6)?;
            if h.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(UrlError::Ipv6);
            }
            (h, p.strip_prefix(':'))
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        if host.is_empty() {
            return Err(UrlError::NoHost);
        }
        let port = match port {
            Some(p) if !p.is_empty() => p.parse::<u16>().map_err(|_| UrlError::Port)?,
            Some(_) => return Err(UrlError::Port),
            None if https => 443,
            None => 80,
        };
        let path = if path.is_empty() {
            "/".to_string()
        } else if path.starts_with('?') {
            format!("/{path}")
        } else {
            path.to_string()
        };
        Ok(Url {
            https,
            host: host.to_string(),
            port,
            path,
        })
    }

    /// `Host:` header value: brackets for IPv6, port only when it is not the scheme default.
    pub fn host_header(&self) -> String {
        let default = if self.https { 443 } else { 80 };
        let h = host_literal(&self.host);
        if self.port == default {
            h
        } else {
            format!("{h}:{}", self.port)
        }
    }

    /// `host:port` for messages, without the path (which may hold a webhook secret).
    pub fn endpoint(&self) -> String {
        format!(
            "{}://{}",
            if self.https { "https" } else { "http" },
            self.host_header()
        )
    }
}

fn strip_scheme<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    if s.len() >= scheme.len() && s[..scheme.len()].eq_ignore_ascii_case(scheme) {
        Some(&s[scheme.len()..])
    } else {
        None
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.endpoint(), self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        let u = Url::parse("https://ntfy.sh/alerts").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port, u.path.as_str()), (true, "ntfy.sh", 443, "/alerts"));
        assert_eq!(u.host_header(), "ntfy.sh");
        let u = Url::parse("HTTP://h:8443/p?q=1").unwrap();
        assert_eq!((u.https, u.port, u.path.as_str()), (false, 8443, "/p?q=1"));
        assert_eq!(u.host_header(), "h:8443");
        let u = Url::parse("http://[::1]:80").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("::1", 80, "/"));
        assert_eq!(u.host_header(), "[::1]");
        assert_eq!(Url::parse("https://h?x=1").unwrap().path, "/?x=1");
        assert_eq!(Url::parse("ntfy.sh/x"), Err(UrlError::Scheme));
        assert_eq!(Url::parse("https://u:p@h/x"), Err(UrlError::UserInfo));
        assert_eq!(Url::parse("https://h:99999/"), Err(UrlError::Port));
        assert_eq!(Url::parse("https:///x"), Err(UrlError::NoHost));
        assert_eq!(Url::parse("https://[zz]/x"), Err(UrlError::Ipv6));
        assert_eq!(
            Url::parse("https://discord.com/api/webhooks/1/s").unwrap().to_string(),
            "https://discord.com/api/webhooks/1/s"
        );
    }
}
