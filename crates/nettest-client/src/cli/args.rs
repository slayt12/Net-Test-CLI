//! clap definition for the client. Flags override the saved config; a positional target such as
//! `wss://host:9102` or `sip://phone` is accepted as shorthand for `--protocol --host --port`,
//! with the scheme's default port filled in when none is written.

use clap::Parser;
use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::stats::TpDirection;
use nettest_proto::transport::Protocol;

#[derive(Parser, Debug)]
#[command(
    name = "nettest-client",
    version,
    about = "Interactive / scriptable network troubleshooting client: ws, wss, tcp, udp against nettest-server; ping, connect, sip, http, https against any device",
    after_help = "EXIT CODES (headless): 0 ok, 1 thresholds exceeded, 2 connect/auth failure, 3 usage, 4 io, 5 interrupted"
)]
pub struct Args {
    /// Target shorthand: ws://10.0.0.5:9101, tcp://host:9100, ping://10.0.0.7, sip://phone, connect://phone:5060, https://device/
    pub target: Option<String>,

    #[arg(long, env = "NETTEST_HOST")]
    pub host: Option<String>,
    #[arg(long, env = "NETTEST_PORT")]
    pub port: Option<u16>,
    #[arg(long, short = 'P', value_enum, env = "NETTEST_PROTOCOL")]
    pub protocol: Option<Protocol>,
    #[arg(long, short = 'm', value_enum)]
    pub mode: Option<TestMode>,

    /// Probe interval, e.g. 200ms, 1s
    #[arg(long, short = 'i')]
    pub interval: Option<humantime::Duration>,
    /// Frame size on the wire in bytes, header included (min 32)
    #[arg(long, short = 's')]
    pub size: Option<u32>,
    /// Stop after this long, e.g. 30s, 5m, 2h (0 = unlimited)
    #[arg(long, short = 'd')]
    pub duration: Option<humantime::Duration>,
    /// Stop after this many probes (0 = unlimited)
    #[arg(long, short = 'c')]
    pub count: Option<u64>,

    #[arg(long, env = "NETTEST_TOKEN")]
    pub token: Option<String>,
    /// Accept any TLS certificate (wss)
    #[arg(long)]
    pub insecure: bool,
    /// Pin the server certificate by SHA-256 fingerprint (wss)
    #[arg(long)]
    pub fingerprint: Option<String>,
    /// Connect timeout, e.g. 5s
    #[arg(long)]
    pub timeout: Option<humantime::Duration>,
    /// Heartbeat interval in soak mode
    #[arg(long)]
    pub heartbeat: Option<humantime::Duration>,
    /// Seconds per direction in throughput mode
    #[arg(long)]
    pub tp_secs: Option<u64>,
    /// Bytes per throughput frame
    #[arg(long)]
    pub tp_chunk: Option<u32>,
    #[arg(long, value_enum)]
    pub tp_direction: Option<TpDirection>,
    /// Force IPv4
    #[arg(short = '4', conflicts_with = "ipv6")]
    pub ipv4: bool,
    /// Force IPv6
    #[arg(short = '6')]
    pub ipv6: bool,
    /// WebSocket request path, or the http/https probe path
    #[arg(long, visible_alias = "path")]
    pub ws_path: Option<String>,
    /// Serverless soak: consecutive failed probes that count as an outage
    #[arg(long)]
    pub outage_after: Option<u32>,

    /// Print every sample to the console, not just failures
    #[arg(long)]
    pub all_samples: bool,
    /// Disable console sample/event output
    #[arg(long)]
    pub quiet: bool,
    /// Write a CSV log (optionally to this path)
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub csv: Option<String>,
    /// Write a plain-text log (optionally to this path)
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub text: Option<String>,
    /// Write a JSON Lines log (optionally to this path)
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub jsonl: Option<String>,
    /// Write an HTML report at the end (optionally to this path)
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub report: Option<String>,
    /// Do not write an HTML report
    #[arg(long, conflicts_with = "report")]
    pub no_report: bool,
    /// Directory for generated files
    #[arg(long, short = 'o')]
    pub out_dir: Option<String>,

    /// Run without the interactive terminal UI
    #[arg(long)]
    pub no_tui: bool,
    /// Fail (exit 1) if loss exceeds this percentage (headless)
    #[arg(long, default_value_t = 0.0)]
    pub max_loss: f64,
    /// Fail (exit 1) if p95 RTT exceeds this many ms (headless)
    #[arg(long)]
    pub max_p95_ms: Option<f64>,
    /// Print the final summary as one JSON line on stdout (headless)
    #[arg(long)]
    pub json_summary: bool,

    /// Read settings from this TOML file instead of the default location
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
    /// Write the effective settings back to the config file
    #[arg(long)]
    pub save_config: bool,
}

impl Args {
    pub fn apply(&self, mut cfg: ClientConfig) -> Result<ClientConfig, String> {
        if let Some(t) = &self.target {
            parse_target(t, &mut cfg)?;
        }
        if let Some(v) = &self.host {
            cfg.host = v.clone();
        }
        if let Some(v) = self.port {
            cfg.port = v;
        }
        if let Some(v) = self.protocol {
            cfg.protocol = v;
        }
        if let Some(v) = self.mode {
            cfg.mode = v;
        }
        if let Some(v) = self.interval {
            cfg.interval_ms = v.as_millis().max(1) as u64;
        }
        if let Some(v) = self.size {
            cfg.payload_bytes = v;
        }
        if let Some(v) = self.duration {
            cfg.duration_secs = v.as_secs();
        }
        if let Some(v) = self.count {
            cfg.count = v;
        }
        if let Some(v) = &self.token {
            cfg.token = v.clone();
        }
        if self.insecure {
            cfg.insecure = true;
        }
        if let Some(v) = &self.fingerprint {
            cfg.fingerprint = v.clone();
        }
        if let Some(v) = self.timeout {
            cfg.connect_timeout_secs = v.as_secs().max(1);
        }
        if let Some(v) = self.heartbeat {
            cfg.heartbeat_ms = v.as_millis().max(100) as u64;
        }
        if let Some(v) = self.tp_secs {
            cfg.tp_secs = v.max(1);
        }
        if let Some(v) = self.tp_chunk {
            cfg.tp_chunk_bytes = v.max(1);
        }
        if let Some(v) = self.tp_direction {
            cfg.tp_direction = v;
        }
        if self.ipv4 {
            cfg.ipv6 = Some(false);
        }
        if self.ipv6 {
            cfg.ipv6 = Some(true);
        }
        if let Some(v) = &self.ws_path {
            cfg.ws_path = v.clone();
        }
        if let Some(v) = self.outage_after {
            cfg.outage_after = v.max(1);
        }
        if self.all_samples {
            cfg.sinks.console_failures_only = false;
        }
        if self.quiet {
            cfg.sinks.console = false;
        }
        if self.csv.is_some() {
            cfg.sinks.csv = true;
        }
        if self.text.is_some() {
            cfg.sinks.text = true;
        }
        if self.jsonl.is_some() {
            cfg.sinks.jsonl = true;
        }
        if self.report.is_some() {
            cfg.sinks.html_report = true;
        }
        if self.no_report {
            cfg.sinks.html_report = false;
        }
        if let Some(v) = &self.out_dir {
            cfg.sinks.output_dir = v.clone();
        }
        if cfg.payload_bytes < nettest_proto::frame::HEADER_LEN as u32 {
            return Err(format!(
                "--size must be at least {} bytes (the frame header)",
                nettest_proto::frame::HEADER_LEN
            ));
        }
        if cfg.mode == TestMode::Throughput && cfg.protocol.is_serverless() {
            return Err(format!(
                "throughput needs a nettest-server on the far end; {} is a serverless probe, use -m latency or -m soak",
                cfg.protocol
            ));
        }
        if cfg.protocol == Protocol::Connect && cfg.port == 0 {
            return Err("connect:// needs a port, e.g. connect://host:5060".into());
        }
        Ok(cfg)
    }
}

/// `scheme://host:port`, `scheme://host` (scheme default port), `host:port`, or `[v6]:port`.
fn parse_target(t: &str, cfg: &mut ClientConfig) -> Result<(), String> {
    let (scheme, rest) = match t.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, t),
    };
    if let Some(s) = scheme {
        cfg.protocol = s.parse()?;
    }
    // `host:port/path` carries the WebSocket or HTTP request path.
    let rest = match rest.split_once('/') {
        Some((hostport, path)) => {
            if !path.is_empty() {
                cfg.ws_path = format!("/{path}");
            }
            hostport
        }
        None => rest,
    };
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        let (h, p) = r
            .split_once(']')
            .ok_or_else(|| format!("bad IPv6 target '{t}'"))?;
        (h.to_string(), p.strip_prefix(':'))
    } else {
        match rest.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p)),
            _ => (rest.to_string(), None),
        }
    };
    if !host.is_empty() {
        cfg.host = host;
    }
    match (port, scheme) {
        (Some(p), _) => {
            cfg.port = p.parse().map_err(|_| format!("bad port in target '{t}'"))?;
        }
        (None, Some(_)) => {
            if let Some(p) = cfg.protocol.default_port() {
                cfg.port = p;
            } else if cfg.protocol == Protocol::Connect {
                return Err("connect:// needs a port, e.g. connect://host:5060".into());
            }
        }
        (None, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_forms() {
        let mut c = ClientConfig::default();
        parse_target("wss://example.com:9102", &mut c).unwrap();
        assert_eq!(c.protocol, Protocol::Wss);
        assert_eq!(c.host, "example.com");
        assert_eq!(c.port, 9102);
        parse_target("udp://[::1]:7", &mut c).unwrap();
        assert_eq!(c.protocol, Protocol::Udp);
        assert_eq!(c.host, "::1");
        assert_eq!(c.port, 7);
        parse_target("10.0.0.1", &mut c).unwrap();
        assert_eq!(c.host, "10.0.0.1");
        assert_eq!(c.port, 7);
        assert!(parse_target("ftp://x:1", &mut c).is_err());
    }

    #[test]
    fn serverless_schemes_and_default_ports() {
        let mut c = ClientConfig::default();
        parse_target("sip://phone.local", &mut c).unwrap();
        assert_eq!(
            (c.protocol, c.host.as_str(), c.port),
            (Protocol::Sip, "phone.local", 5060)
        );
        parse_target("https://10.0.0.7/", &mut c).unwrap();
        assert_eq!((c.protocol, c.port), (Protocol::Https, 443));
        parse_target("http://10.0.0.7:8080/admin", &mut c).unwrap();
        assert_eq!((c.protocol, c.port), (Protocol::Http, 8080));
        assert_eq!(c.ws_path, "/admin");
        parse_target("ping://[fe80::1]", &mut c).unwrap();
        assert_eq!((c.protocol, c.host.as_str()), (Protocol::Ping, "fe80::1"));
        parse_target("ws://10.0.0.5", &mut c).unwrap();
        assert_eq!(c.port, 9101);
        assert!(parse_target("connect://10.0.0.7", &mut c).is_err());
        parse_target("connect://10.0.0.7:5060", &mut c).unwrap();
        assert_eq!((c.protocol, c.port), (Protocol::Connect, 5060));
    }

    #[test]
    fn throughput_rejected_for_probes() {
        let args = Args::try_parse_from(["nettest-client", "ping://10.0.0.7", "-m", "throughput"])
            .unwrap();
        let err = args.apply(ClientConfig::default()).unwrap_err();
        assert!(err.contains("nettest-server"), "{err}");
        let args =
            Args::try_parse_from(["nettest-client", "sip://10.0.0.7", "--outage-after", "0"])
                .unwrap();
        assert_eq!(args.apply(ClientConfig::default()).unwrap().outage_after, 1);
    }
}
