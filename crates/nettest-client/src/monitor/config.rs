//! `monitor.toml`: the targets to watch, the alert thresholds and the notifiers.
//!
//! The file is parsed into `MonitorConfig` (all strings, so a typo is reported with its field
//! name) and then *resolved* into runtime types: one `ClientConfig` per target, built through
//! the same `parse_target` the CLI uses, so every target form the CLI accepts works here too.
//! All validation errors are collected and reported at once.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nettest_proto::config::{ClientConfig, SinkConfig, TestMode};
use nettest_proto::http::Url;
use nettest_proto::tls::client::TlsClientMode;
use nettest_proto::transport::Protocol;
use serde::{Deserialize, Serialize};

use super::health::Thresholds;
use crate::cli::args::parse_target;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MonitorConfig {
    pub monitor: MonitorSection,
    pub targets: Vec<TargetConfig>,
    pub notify: Vec<NotifyConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MonitorSection {
    /// Append human-readable log lines here ("" = stderr only, or whatever --log-file says).
    pub log_file: String,
    /// Append JSON Lines log here ("" = off).
    pub jsonl_file: String,
    /// Periodic per-target status line, e.g. "1h"; "0" disables it.
    pub summary_every: String,
    /// Send a notification when the monitor starts (useful to confirm the webhooks work).
    pub notify_on_start: bool,
    /// Name of this machine in alert text ("" = detect).
    pub hostname: String,
}

impl Default for MonitorSection {
    fn default() -> Self {
        Self {
            log_file: String::new(),
            jsonl_file: String::new(),
            summary_every: "1h".into(),
            notify_on_start: false,
            hostname: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    /// Label used in alerts and the log (default: the target string).
    #[serde(default)]
    pub name: String,
    /// Same syntax as the CLI positional: ping://10.0.0.7, ws://host:9101, sip://pbx, ...
    pub target: String,
    #[serde(default = "d_interval")]
    pub interval: String,
    /// Per-probe / connect timeout; a probe unanswered this long counts as a failure.
    #[serde(default = "d_timeout")]
    pub timeout: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub fingerprint: String,
    /// WebSocket request path or http(s) probe path ("" = from the target URL or "/").
    #[serde(default)]
    pub ws_path: String,
    /// true = IPv6 only, false = IPv4 only, unset = either.
    #[serde(default)]
    pub ipv6: Option<bool>,
    #[serde(default)]
    pub payload_bytes: Option<u32>,
    #[serde(default = "d_three")]
    pub failures_before_down: u32,
    #[serde(default = "d_one")]
    pub successes_before_up: u32,
    /// Re-send the DOWN alert at this cadence while still down; "0" = never.
    #[serde(default = "d_zero")]
    pub remind_every: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    Ntfy,
    Slack,
    Discord,
}

impl std::fmt::Display for NotifyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotifyKind::Ntfy => "ntfy",
            NotifyKind::Slack => "slack",
            NotifyKind::Discord => "discord",
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    pub kind: NotifyKind,
    /// ntfy: topic URL (https://ntfy.sh/<topic>); Slack / Discord: the incoming-webhook URL.
    pub url: String,
    #[serde(default)]
    pub name: String,
    /// ntfy access token (sent as `Authorization: Bearer`).
    #[serde(default)]
    pub token: String,
    /// ntfy priority for DOWN alerts: 1-5 or min/low/default/high/urgent/max.
    #[serde(default = "d_priority")]
    pub priority: String,
    /// ntfy tags (emoji short codes) for DOWN / UP alerts.
    #[serde(default = "d_tags_down")]
    pub tags_down: String,
    #[serde(default = "d_tags_up")]
    pub tags_up: String,
    /// Accept any TLS certificate (self-hosted ntfy with a self-signed certificate).
    #[serde(default)]
    pub insecure: bool,
    /// Pin the endpoint's certificate by SHA-256 instead.
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default = "d_notify_timeout")]
    pub timeout: String,
}

fn d_interval() -> String {
    "10s".into()
}
fn d_timeout() -> String {
    "5s".into()
}
fn d_three() -> u32 {
    3
}
fn d_one() -> u32 {
    1
}
fn d_zero() -> String {
    "0".into()
}
fn d_priority() -> String {
    "high".into()
}
fn d_tags_down() -> String {
    "warning,red_circle".into()
}
fn d_tags_up() -> String {
    "white_check_mark".into()
}
fn d_notify_timeout() -> String {
    "10s".into()
}

// ---- resolved form ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub name: String,
    pub client: ClientConfig,
    pub interval: Duration,
    pub timeout: Duration,
    pub thresholds: Thresholds,
}

#[derive(Debug, Clone)]
pub struct ResolvedNotifier {
    pub kind: NotifyKind,
    pub name: String,
    pub url: Url,
    pub token: String,
    pub priority: String,
    pub tags_down: String,
    pub tags_up: String,
    pub tls: TlsClientMode,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub log_file: Option<PathBuf>,
    pub jsonl_file: Option<PathBuf>,
    pub summary_every: Option<Duration>,
    pub notify_on_start: bool,
    pub hostname: String,
    pub targets: Vec<ResolvedTarget>,
    pub notifiers: Vec<ResolvedNotifier>,
    /// Non-fatal findings (no notifiers, odd priority) for the log.
    pub warnings: Vec<String>,
}

/// Read and parse. A missing file is an error here (unlike the client's own config, where
/// defaults are fine): a monitor without targets is pointless.
pub fn load(path: &Path) -> Result<MonitorConfig, String> {
    if !path.exists() {
        return Err(format!(
            "{} does not exist (create one with `nettest-client monitor --example-config > {}`)",
            path.display(),
            path.display()
        ));
    }
    let text = read_text(path)?;
    parse_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Read a config file written by any common tool: UTF-8 with or without a byte-order mark, or
/// UTF-16 with one. Windows PowerShell 5.1 writes `>` redirections as UTF-16 LE, so
/// `nettest-client monitor --example-config > monitor.toml` would otherwise be unreadable.
pub fn read_text(path: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    decode_text(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |le: bool| -> Result<String, String> {
        let body = &bytes[2..];
        if body.len() % 2 != 0 {
            return Err("UTF-16 file has an odd number of bytes".into());
        }
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16(&units).map_err(|_| "UTF-16 file contains invalid characters".into())
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16(true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return utf16(false);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(body.to_vec()).map_err(|_| {
        "not valid UTF-8 (or UTF-16 with a byte-order mark); save the file as UTF-8, e.g. in PowerShell `(Get-Content monitor.toml) | Set-Content -Encoding utf8 monitor.toml`".into()
    })
}

pub fn parse_str(text: &str) -> Result<MonitorConfig, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Turn the file into runtime types, collecting every problem.
pub fn resolve(cfg: &MonitorConfig) -> Result<Resolved, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if cfg.targets.is_empty() {
        errors.push("no [[targets]] configured".to_string());
    }
    let mut targets = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, t) in cfg.targets.iter().enumerate() {
        let label = if t.name.trim().is_empty() {
            t.target.clone()
        } else {
            t.name.trim().to_string()
        };
        let ctx = format!("targets[{i}] ({label})");
        if label.is_empty() {
            errors.push(format!("{ctx}: target is empty"));
            continue;
        }
        if !seen.insert(label.clone()) {
            errors.push(format!("{ctx}: duplicate name"));
        }
        let mut c = ClientConfig {
            mode: TestMode::Latency,
            sinks: SinkConfig {
                console: false,
                html_report: false,
                ..SinkConfig::default()
            },
            ..ClientConfig::default()
        };
        if let Err(e) = parse_target(&t.target, &mut c) {
            errors.push(format!("{ctx}: {e}"));
            continue;
        }
        let interval = match parse_dur(&t.interval) {
            Ok(d) if d >= Duration::from_millis(100) => d,
            Ok(_) => {
                errors.push(format!("{ctx}: interval must be at least 100ms"));
                continue;
            }
            Err(e) => {
                errors.push(format!("{ctx}: interval: {e}"));
                continue;
            }
        };
        let timeout = match parse_dur(&t.timeout) {
            Ok(d) if !d.is_zero() => d,
            Ok(_) => {
                errors.push(format!("{ctx}: timeout must be greater than 0"));
                continue;
            }
            Err(e) => {
                errors.push(format!("{ctx}: timeout: {e}"));
                continue;
            }
        };
        let remind = match parse_dur(&t.remind_every) {
            Ok(d) => (!d.is_zero()).then_some(d),
            Err(e) => {
                errors.push(format!("{ctx}: remind_every: {e}"));
                continue;
            }
        };
        if t.failures_before_down == 0 {
            errors.push(format!("{ctx}: failures_before_down must be at least 1"));
        }
        if t.successes_before_up == 0 {
            errors.push(format!("{ctx}: successes_before_up must be at least 1"));
        }
        c.interval_ms = interval.as_millis() as u64;
        c.connect_timeout_secs = timeout.as_secs().max(1);
        c.loss_timeout_ms = timeout.as_millis() as u64;
        c.duration_secs = 0;
        c.count = 0;
        c.token = t.token.clone();
        c.insecure = t.insecure;
        c.fingerprint = t.fingerprint.trim().to_string();
        if !t.ws_path.trim().is_empty() {
            c.ws_path = t.ws_path.trim().to_string();
        }
        c.ipv6 = t.ipv6;
        if let Some(p) = t.payload_bytes {
            c.payload_bytes = p;
        }
        c.outage_after = t.failures_before_down.max(1);
        if c.payload_bytes < nettest_proto::frame::HEADER_LEN as u32 {
            errors.push(format!(
                "{ctx}: payload_bytes must be at least {}",
                nettest_proto::frame::HEADER_LEN
            ));
        }
        if !c.fingerprint.is_empty()
            && let Err(e) = nettest_proto::tls::fingerprint::parse(&c.fingerprint)
        {
            errors.push(format!("{ctx}: fingerprint: {e}"));
        }
        if c.protocol == Protocol::Wss && !c.insecure && c.fingerprint.is_empty() {
            errors.push(format!(
                "{ctx}: wss needs `insecure = true` or `fingerprint = \"<sha256>\"` (nettest-server certificates are self-signed)"
            ));
        }
        if c.protocol == Protocol::Connect && c.port == 0 {
            errors.push(format!("{ctx}: connect:// needs a port"));
        }
        targets.push(ResolvedTarget {
            name: label,
            client: c,
            interval,
            timeout,
            thresholds: Thresholds {
                down_after: t.failures_before_down.max(1),
                up_after: t.successes_before_up.max(1),
                remind_every: remind,
            },
        });
    }

    let mut notifiers = Vec::new();
    for (i, n) in cfg.notify.iter().enumerate() {
        let name = if n.name.trim().is_empty() {
            format!("{}#{}", n.kind, i + 1)
        } else {
            n.name.trim().to_string()
        };
        let ctx = format!("notify[{i}] ({name})");
        let url = match Url::parse(&n.url) {
            Ok(u) => u,
            Err(e) => {
                errors.push(format!("{ctx}: url: {e}"));
                continue;
            }
        };
        let timeout = match parse_dur(&n.timeout) {
            Ok(d) if !d.is_zero() => d,
            Ok(_) => {
                errors.push(format!("{ctx}: timeout must be greater than 0"));
                continue;
            }
            Err(e) => {
                errors.push(format!("{ctx}: timeout: {e}"));
                continue;
            }
        };
        let tls = if !n.fingerprint.trim().is_empty() {
            match nettest_proto::tls::fingerprint::parse(n.fingerprint.trim()) {
                Ok(fp) => TlsClientMode::Pinned(fp),
                Err(e) => {
                    errors.push(format!("{ctx}: fingerprint: {e}"));
                    continue;
                }
            }
        } else if n.insecure {
            TlsClientMode::Insecure
        } else {
            TlsClientMode::WebPki
        };
        if !url.https && (n.insecure || !n.fingerprint.trim().is_empty()) {
            warnings.push(format!("{ctx}: insecure/fingerprint have no effect on a plain http:// url"));
        }
        if n.kind == NotifyKind::Ntfy && !valid_ntfy_priority(&n.priority) {
            warnings.push(format!(
                "{ctx}: priority '{}' is not an ntfy priority (1-5, min, low, default, high, urgent, max)",
                n.priority
            ));
        }
        if n.kind != NotifyKind::Ntfy && !n.token.is_empty() {
            warnings.push(format!("{ctx}: token is only used by ntfy"));
        }
        notifiers.push(ResolvedNotifier {
            kind: n.kind,
            name,
            url,
            token: n.token.trim().to_string(),
            priority: n.priority.trim().to_string(),
            tags_down: n.tags_down.trim().to_string(),
            tags_up: n.tags_up.trim().to_string(),
            tls,
            timeout,
        });
    }
    if cfg.notify.is_empty() {
        warnings.push("no [[notify]] configured: transitions are logged only".to_string());
    }

    let summary_every = match parse_dur(&cfg.monitor.summary_every) {
        Ok(d) => (!d.is_zero()).then_some(d.max(Duration::from_secs(10))),
        Err(e) => {
            errors.push(format!("[monitor] summary_every: {e}"));
            None
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Resolved {
        log_file: non_empty_path(&cfg.monitor.log_file),
        jsonl_file: non_empty_path(&cfg.monitor.jsonl_file),
        summary_every,
        notify_on_start: cfg.monitor.notify_on_start,
        hostname: if cfg.monitor.hostname.trim().is_empty() {
            super::local_hostname()
        } else {
            cfg.monitor.hostname.trim().to_string()
        },
        targets,
        notifiers,
        warnings,
    })
}

fn non_empty_path(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// humantime syntax (`10s`, `2m`, `1h30m`), plus a bare `0`.
pub fn parse_dur(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s == "0" || s.is_empty() {
        return Ok(Duration::ZERO);
    }
    humantime::parse_duration(s).map_err(|e| format!("'{s}': {e}"))
}

fn valid_ntfy_priority(p: &str) -> bool {
    matches!(
        p.trim(),
        "1" | "2" | "3" | "4" | "5" | "min" | "low" | "default" | "high" | "urgent" | "max"
    )
}

/// Human summary for `--check` and `service status`.
pub fn describe(r: &Resolved) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("hostname    {}", r.hostname));
    out.push(format!(
        "log file    {}",
        r.log_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(stderr only)".into())
    ));
    out.push(format!(
        "summary     {}",
        r.summary_every
            .map(|d| format!("every {}", super::fmt_duration(d)))
            .unwrap_or_else(|| "off".into())
    ));
    out.push(format!("targets     {}", r.targets.len()));
    for t in &r.targets {
        out.push(format!(
            "  {:<20} {:<36} every {:<6} timeout {:<6} down after {}  up after {}{}",
            t.name,
            t.client.target(),
            super::fmt_duration(t.interval),
            super::fmt_duration(t.timeout),
            t.thresholds.down_after,
            t.thresholds.up_after,
            t.thresholds
                .remind_every
                .map(|d| format!("  remind {}", super::fmt_duration(d)))
                .unwrap_or_default()
        ));
    }
    out.push(format!("notifiers   {}", r.notifiers.len()));
    for n in &r.notifiers {
        out.push(format!(
            "  {:<20} {:<8} {}{}",
            n.name,
            n.kind,
            n.url.endpoint(),
            match n.tls {
                TlsClientMode::WebPki => "",
                TlsClientMode::Insecure => "  (insecure)",
                TlsClientMode::Pinned(_) => "  (pinned)",
            }
        ));
    }
    out
}

pub fn example_toml() -> &'static str {
    r##"# nettest-client monitor configuration.
#
# Run in the foreground:   nettest-client monitor --config monitor.toml
# Validate:                nettest-client monitor --config monitor.toml --check
# Test the webhooks:       nettest-client monitor --config monitor.toml --test-notify
# Install as a service:    nettest-client service install --from monitor.toml
#
# Durations use humantime syntax: 500ms, 10s, 2m, 1h30m. "0" disables.

[monitor]
# log_file = "/var/log/nettest-monitor/monitor.log"   # the service sets this itself
summary_every = "1h"          # one status line per target in the log; "0" = off
notify_on_start = false       # send a "monitor started" message (confirms the webhooks work)
# hostname = "office-pc"      # name used in alert text (default: detected)

# One [[targets]] block per endpoint. `target` uses the same forms as the command line:
#   ping://10.0.0.7            ICMP echo
#   connect://10.0.0.7:5060    TCP handshake only
#   sip://pbx.example.com      SIP OPTIONS over UDP (port 5060 unless given)
#   http://10.0.0.7/           HTTP HEAD (any status counts as alive)
#   https://device.local/      HTTPS HEAD (self-signed accepted unless `fingerprint` is set)
#   ws://host:9101  wss://host:9102  tcp://host:9100  udp://host:9100   nettest-server echo
[[targets]]
name = "gateway"
target = "ping://10.0.0.1"
interval = "5s"
timeout = "2s"
failures_before_down = 3      # DOWN after this many consecutive failures
successes_before_up = 1       # UP after this many consecutive successes
remind_every = "0"            # re-send the DOWN alert while still down, e.g. "1h"

[[targets]]
name = "pbx"
target = "sip://pbx.example.com"
interval = "10s"
timeout = "5s"

[[targets]]
name = "nettest wss"
target = "wss://nettest.example.com:9102"
token = "the-server-token"
insecure = true               # or fingerprint = "<sha256 printed by nettest-server>"
interval = "10s"
timeout = "5s"
failures_before_down = 2

# Where alerts go. Any number of notifiers, all receive every alert.
[[notify]]
kind = "ntfy"
name = "ops phone"
url = "https://ntfy.sh/my-secret-topic"
# token = "tk_..."            # ntfy access token (Authorization: Bearer)
priority = "high"             # ntfy priority for DOWN alerts
tags_down = "warning,red_circle"
tags_up = "white_check_mark"
# insecure = true             # self-hosted ntfy with a self-signed certificate
# fingerprint = "<sha256>"    # or pin it

# [[notify]]
# kind = "slack"
# name = "#noc"
# url = "https://hooks.slack.com/services/T000/B000/XXXX"

# [[notify]]
# kind = "discord"
# name = "alerts channel"
# url = "https://discord.com/api/webhooks/123456/abcdef"
"##
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_resolves_cleanly() {
        let cfg = parse_str(example_toml()).unwrap();
        let r = resolve(&cfg).unwrap();
        assert_eq!(r.targets.len(), 3);
        assert_eq!(r.notifiers.len(), 1);
        let gw = &r.targets[0];
        assert_eq!(gw.name, "gateway");
        assert_eq!(gw.client.protocol, Protocol::Ping);
        assert_eq!(gw.client.interval_ms, 5000);
        assert_eq!(gw.client.loss_timeout_ms, 2000);
        assert_eq!(gw.client.outage_after, 3);
        assert!(!gw.client.sinks.console && !gw.client.sinks.html_report);
        assert_eq!(gw.thresholds.remind_every, None);
        let wss = &r.targets[2];
        assert_eq!((wss.client.protocol, wss.client.port), (Protocol::Wss, 9102));
        assert_eq!(wss.client.token, "the-server-token");
        assert_eq!(wss.thresholds.down_after, 2);
        assert_eq!(r.notifiers[0].tls, TlsClientMode::WebPki);
        assert_eq!(r.notifiers[0].url.path, "/my-secret-topic");
        assert_eq!(r.summary_every, Some(Duration::from_secs(3600)));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn errors_are_collected() {
        let cfg = parse_str(
            r#"
[[targets]]
name = "a"
target = "wss://h:9102"
[[targets]]
name = "a"
target = "connect://h"
interval = "soon"
[[targets]]
target = "ping://10.0.0.1"
failures_before_down = 0
[[notify]]
kind = "slack"
url = "hooks.slack.com/x"
"#,
        )
        .unwrap();
        let errs = resolve(&cfg).unwrap_err();
        let joined = errs.join("\n");
        assert!(joined.contains("wss needs"), "{joined}");
        assert!(joined.contains("duplicate name"), "{joined}");
        assert!(joined.contains("connect:// needs a port"), "{joined}");
        assert!(joined.contains("failures_before_down"), "{joined}");
        assert!(joined.contains("url: url must start with http"), "{joined}");
        assert!(!joined.contains("interval"), "a target that already failed to parse stops there: {joined}");
    }

    #[test]
    fn unknown_keys_and_minimal_file() {
        assert!(parse_str("[[targets]]\ntarget = \"ping://x\"\ninterval_ms = 5\n").is_err());
        let cfg = parse_str("[[targets]]\ntarget = \"ping://10.0.0.1\"\n").unwrap();
        let r = resolve(&cfg).unwrap();
        assert_eq!(r.targets[0].name, "ping://10.0.0.1");
        assert_eq!(r.targets[0].thresholds, Thresholds::default());
        assert_eq!(r.warnings.len(), 1, "warns about the missing notifiers");
        assert!(resolve(&MonitorConfig::default()).is_err());
    }

    #[test]
    fn text_encodings() {
        let toml = "[[targets]]\ntarget = \"ping://10.0.0.1\"\n";
        assert_eq!(decode_text(toml.as_bytes()).unwrap(), toml);
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend_from_slice(toml.as_bytes());
        assert_eq!(decode_text(&bom8).unwrap(), toml);
        // What Windows PowerShell 5.1 writes for `--example-config > monitor.toml`.
        let mut le = vec![0xFF, 0xFE];
        for u in toml.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&le).unwrap(), toml);
        let mut be = vec![0xFE, 0xFF];
        for u in toml.encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_text(&be).unwrap(), toml);
        let err = decode_text(&[0xC3, 0x28, b'x']).unwrap_err();
        assert!(err.contains("Set-Content"), "{err}");
        assert!(decode_text(&[0xFF, 0xFE, 0x41]).is_err());
    }

    #[test]
    fn notifier_tls_modes_and_warnings() {
        let cfg = parse_str(
            r#"
[[targets]]
target = "ping://10.0.0.1"
[[notify]]
kind = "ntfy"
url = "http://ntfy.lan/t"
insecure = true
priority = "loud"
[[notify]]
kind = "discord"
url = "https://discord.com/api/webhooks/1/x"
token = "x"
"#,
        )
        .unwrap();
        let r = resolve(&cfg).unwrap();
        assert_eq!(r.notifiers[0].tls, TlsClientMode::Insecure);
        assert_eq!(r.notifiers[0].name, "ntfy#1");
        assert_eq!(r.notifiers[1].tls, TlsClientMode::WebPki);
        assert_eq!(r.warnings.len(), 3, "{:?}", r.warnings);
        assert_eq!(parse_dur("0").unwrap(), Duration::ZERO);
        assert_eq!(parse_dur("1h30m").unwrap(), Duration::from_secs(5400));
        assert!(parse_dur("10").is_err());
    }
}
