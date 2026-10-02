//! clap definition for the server: flat flags for a foreground run, plus the `service`
//! subcommand that installs the same program as a system service. `ServerFlags` is flattened
//! into both so every setting can be given at install or edit time with identical names.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use nettest_proto::config::ServerConfig;

/// Settings that end up in `ServerConfig`. Precedence: flag > `NETTEST_*` env > file > default.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct ServerFlags {
    /// Bind address: an IP, or `any` for both 0.0.0.0 and [::]
    #[arg(long, env = "NETTEST_BIND")]
    pub bind: Option<String>,
    /// Raw TCP echo port (0 disables)
    #[arg(long, env = "NETTEST_TCP_PORT")]
    pub tcp_port: Option<u16>,
    /// UDP echo port (0 disables)
    #[arg(long, env = "NETTEST_UDP_PORT")]
    pub udp_port: Option<u16>,
    /// WebSocket (ws://) port (0 disables)
    #[arg(long, env = "NETTEST_WS_PORT")]
    pub ws_port: Option<u16>,
    /// WebSocket over TLS (wss://) port (0 disables)
    #[arg(long, env = "NETTEST_WSS_PORT")]
    pub wss_port: Option<u16>,
    /// Pre-shared token clients must present (empty = open; required for a service)
    #[arg(long, env = "NETTEST_TOKEN")]
    pub token: Option<String>,
    /// Append human-readable log lines to this file
    #[arg(long)]
    pub log_file: Option<String>,
    /// Append JSON Lines log to this file
    #[arg(long)]
    pub jsonl: Option<String>,
    /// Directory holding cert.pem / key.pem (generated on first run)
    #[arg(long)]
    pub cert_dir: Option<String>,
    /// Drop a session after this many seconds without traffic
    #[arg(long)]
    pub idle_timeout: Option<u64>,
    /// Identification text returned to port scanners and browsers on every listener
    #[arg(long, env = "NETTEST_BANNER")]
    pub banner: Option<String>,
}

impl ServerFlags {
    pub fn apply(&self, mut cfg: ServerConfig) -> ServerConfig {
        if let Some(v) = &self.bind {
            cfg.bind = v.clone();
        }
        if let Some(v) = self.tcp_port {
            cfg.tcp_port = v;
        }
        if let Some(v) = self.udp_port {
            cfg.udp_port = v;
        }
        if let Some(v) = self.ws_port {
            cfg.ws_port = v;
        }
        if let Some(v) = self.wss_port {
            cfg.wss_port = v;
        }
        if let Some(v) = &self.token {
            cfg.token = v.clone();
        }
        if let Some(v) = &self.log_file {
            cfg.log_file = v.clone();
        }
        if let Some(v) = &self.jsonl {
            cfg.jsonl_file = v.clone();
        }
        if let Some(v) = &self.cert_dir {
            cfg.cert_dir = v.clone();
        }
        if let Some(v) = self.idle_timeout {
            cfg.idle_timeout_secs = v;
        }
        if let Some(v) = &self.banner {
            cfg.banner = v.clone();
        }
        cfg
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "nettest-server",
    version,
    about = "Echo/soak/throughput server for nettest (tcp, udp, ws, wss); `service` installs it as a system service",
    long_about = None
)]
pub struct Args {
    #[command(flatten)]
    pub flags: ServerFlags,
    /// Run without the interactive terminal UI (log to stderr)
    #[arg(long)]
    pub no_tui: bool,
    /// Read settings from this TOML file instead of the default location
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Write the effective settings back to the config file and continue
    #[arg(long)]
    pub save_config: bool,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Install, edit or control nettest-server as a system service (systemd or Windows)
    Service(crate::service::ServiceArgs),
}
