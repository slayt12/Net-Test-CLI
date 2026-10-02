//! Server settings.

use serde::{Deserialize, Serialize};

/// Identification text sent to anything that connects to a listener without speaking the nettest
/// protocol (port scanners, browsers). Security scanners need a positive identification; the
/// text is configurable but a listener can never be silent.
pub const DEFAULT_BANNER: &str = "nettest by Slaytons Technology Services";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// `any` binds both 0.0.0.0 and [::].
    pub bind: String,
    /// 0 disables a listener.
    pub tcp_port: u16,
    pub udp_port: u16,
    pub ws_port: u16,
    pub wss_port: u16,
    pub token: String,
    pub log_file: String,
    pub jsonl_file: String,
    pub cert_dir: String,
    /// Seconds without any frame before a session is dropped.
    pub idle_timeout_secs: u64,
    pub max_payload_bytes: u32,
    /// Text returned to non-nettest connections on every port; see `DEFAULT_BANNER`.
    pub banner: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "any".into(),
            tcp_port: 9100,
            udp_port: 9100,
            ws_port: 9101,
            wss_port: 9102,
            token: String::new(),
            log_file: String::new(),
            jsonl_file: String::new(),
            cert_dir: String::new(),
            idle_timeout_secs: 60,
            max_payload_bytes: 16 * 1024 * 1024,
            banner: DEFAULT_BANNER.into(),
        }
    }
}

impl ServerConfig {
    /// The banner as sent on the wire: configured text (or the default when blanked) plus the
    /// server version, newline terminated.
    pub fn banner_line(&self) -> String {
        let text = if self.banner.trim().is_empty() {
            DEFAULT_BANNER
        } else {
            self.banner.trim()
        };
        format!("{text} (nettest-server {})\n", env!("CARGO_PKG_VERSION"))
    }
}
