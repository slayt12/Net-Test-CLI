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
    /// Microsoft Teams through a Workflows (Power Automate) "When a Teams webhook request is
    /// received" trigger; the retired Office 365 connector URLs are not supported.
    Teams,
}

impl std::fmt::Display for NotifyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotifyKind::Ntfy => "ntfy",
            NotifyKind::Slack => "slack",
            NotifyKind::Discord => "discord",
            NotifyKind::Teams => "teams",
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    pub kind: NotifyKind,
    /// ntfy: topic URL (https://ntfy.sh/<topic>); Slack / Discord: the incoming-webhook URL;
    /// Teams: the Workflows trigger URL (its `sig=` query parameter is the secret).
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

/// A blank target with the file's defaults; `target` must still be filled in. Used by the TUI
/// builder when a target is added.
impl Default for TargetConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            target: String::new(),
            interval: d_interval(),
            timeout: d_timeout(),
            token: String::new(),
            insecure: false,
            fingerprint: String::new(),
            ws_path: String::new(),
            ipv6: None,
            payload_bytes: None,
            failures_before_down: d_three(),
            successes_before_up: d_one(),
            remind_every: d_zero(),
        }
    }
}

/// A blank ntfy notifier with the file's defaults; `url` must still be filled in.
impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            kind: NotifyKind::Ntfy,
            url: String::new(),
            name: String::new(),
            token: String::new(),
            priority: d_priority(),
            tags_down: d_tags_down(),
            tags_up: d_tags_up(),
            insecure: false,
            fingerprint: String::new(),
            timeout: d_notify_timeout(),
        }
    }
}

impl NotifyKind {
    pub const ALL: [NotifyKind; 4] = [
        NotifyKind::Ntfy,
        NotifyKind::Slack,
        NotifyKind::Discord,
        NotifyKind::Teams,
    ];
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Serialise for writing by the TUI builder. `toml` cannot keep comments, so a short header
/// points at `--example-config` for the annotated form; every key is written out, defaults
/// included, which keeps the file self-documenting.
pub fn to_toml(cfg: &MonitorConfig) -> Result<String, String> {
    let body = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    Ok(format!(
        "# nettest monitor configuration, written by nettest-client {} on {}.\n\
         # Every key is listed; `nettest-client monitor --example-config` explains them.\n\n{body}",
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ))
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
    /// Encoding the file was read in (`Utf8` when resolved from an in-memory string).
    pub encoding: Encoding,
}

/// Warning text for a file that had to be converted; put first in `Resolved::warnings`.
pub fn encoding_warning(encoding: Encoding) -> String {
    format!(
        "config file is {encoding}; converted to UTF-8 while loading (service install stores the installed copy as UTF-8)"
    )
}

/// Read and parse. A missing file is an error here (unlike the client's own config, where
/// defaults are fine): a monitor without targets is pointless. Also returns the encoding the
/// file was found in so callers can tell the user when it was not UTF-8.
pub fn load(path: &Path) -> Result<(MonitorConfig, Encoding), String> {
    if !path.exists() {
        return Err(format!(
            "{} does not exist (create one with `nettest-client monitor --example-config > {}`)",
            path.display(),
            path.display()
        ));
    }
    let d = read_text(path)?;
    let cfg = parse_str(&d.text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((cfg, d.encoding))
}

/// Encoding a config file was found in. Only `Utf8` needs no conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Utf16LeNoBom,
    Utf16BeNoBom,
    Windows1252,
}

impl Encoding {
    pub fn is_utf8(&self) -> bool {
        *self == Encoding::Utf8
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 with byte-order mark",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Utf16LeNoBom => "UTF-16 LE (no byte-order mark)",
            Encoding::Utf16BeNoBom => "UTF-16 BE (no byte-order mark)",
            Encoding::Windows1252 => "Windows-1252 (ANSI)",
        })
    }
}

pub struct Decoded {
    pub text: String,
    pub encoding: Encoding,
}

/// Read a config file written by any common Windows or Linux tool and hand back UTF-8.
/// Windows PowerShell 5.1 writes `>` redirections as UTF-16 LE and `Set-Content` as ANSI
/// (Windows-1252); old Notepad saves ANSI too. Rather than refusing those, every reader converts
/// on the way in and `service install` stores the converted copy, so the service itself only
/// ever sees UTF-8.
pub fn read_text(path: &Path) -> Result<Decoded, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    decode_text(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn decode_text(bytes: &[u8]) -> Result<Decoded, String> {
    let utf16 = |body: &[u8], le: bool| -> Result<String, String> {
        if !body.len().is_multiple_of(2) {
            return Err("UTF-16 file has an odd number of bytes; save the file as UTF-8".into());
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if le { u16::from_le_bytes(c) } else { u16::from_be_bytes(c) })
            .collect();
        let s = String::from_utf16(&units)
            .map_err(|_| "UTF-16 file contains invalid characters; save the file as UTF-8".to_string())?;
        // A second BOM (file concatenation, some editors) would otherwise reach the TOML parser.
        Ok(s.strip_prefix('\u{FEFF}').map(str::to_string).unwrap_or(s))
    };
    let done = |text, encoding| Ok(Decoded { text, encoding });
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) || bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        return Err("UTF-32 is not supported; save the file as UTF-8".into());
    }
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return done(utf16(body, true)?, Encoding::Utf16Le);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return done(utf16(body, false)?, Encoding::Utf16Be);
    }
    if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let s = String::from_utf8(body.to_vec())
            .map_err(|_| "has a UTF-8 byte-order mark but is not valid UTF-8; save the file as UTF-8".to_string())?;
        return done(s, Encoding::Utf8Bom);
    }
    match utf16_without_bom(bytes) {
        Some(true) => return done(utf16(bytes, true)?, Encoding::Utf16LeNoBom),
        Some(false) => return done(utf16(bytes, false)?, Encoding::Utf16BeNoBom),
        None => {}
    }
    match String::from_utf8(bytes.to_vec()) {
        Ok(s) => done(s, Encoding::Utf8),
        Err(_) => done(windows_1252(bytes), Encoding::Windows1252),
    }
}

/// `Some(true)` for UTF-16 LE, `Some(false)` for BE, `None` for anything else. A TOML file is
/// ASCII-dominated, so UTF-16 shows as a NUL in every second byte, and valid UTF-8 text never
/// contains NUL at all. Only the first 512 bytes are inspected.
fn utf16_without_bom(bytes: &[u8]) -> Option<bool> {
    let head = &bytes[..bytes.len().min(512) & !1];
    if head.len() < 4 {
        return None;
    }
    let (even_nul, odd_nul) = head.as_chunks::<2>().0.iter().fold((0usize, 0usize), |(e, o), c| {
        (e + usize::from(c[0] == 0), o + usize::from(c[1] == 0))
    });
    let pairs = head.len() / 2;
    // Every odd byte NUL and no even byte NUL = little endian, and vice versa. A few non-NUL
    // odd bytes would mean non-Latin text; a config file is not expected to have any.
    if odd_nul == pairs && even_nul == 0 {
        Some(true)
    } else if even_nul == pairs && odd_nul == 0 {
        Some(false)
    } else {
        None
    }
}

/// Windows-1252 to UTF-8: 0x00-0x7F and 0xA0-0xFF are Latin-1, 0x80-0x9F are the typographic
/// extras (curly quotes, dashes, €); the five unassigned slots become U+FFFD.
fn windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
        '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
        '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
        '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
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

    let notifiers = resolve_notifiers_into(&cfg.notify, &mut errors, &mut warnings);
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
        encoding: Encoding::Utf8,
    })
}

/// Resolve the `[[notify]]` list on its own, for callers that have no targets (the TUI sends
/// alerts for its own run). `Err` carries every problem; `Ok` carries the notifiers and the
/// non-fatal warnings.
pub fn resolve_notifiers(
    notify: &[NotifyConfig],
) -> Result<(Vec<ResolvedNotifier>, Vec<String>), Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let notifiers = resolve_notifiers_into(notify, &mut errors, &mut warnings);
    if errors.is_empty() {
        Ok((notifiers, warnings))
    } else {
        Err(errors)
    }
}

fn resolve_notifiers_into(
    notify: &[NotifyConfig],
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Vec<ResolvedNotifier> {
    let mut notifiers = Vec::new();
    for (i, n) in notify.iter().enumerate() {
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
        if n.kind == NotifyKind::Teams && !url.https {
            warnings.push(format!("{ctx}: Teams workflow URLs are https://; check the url"));
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
    notifiers
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

# Microsoft Teams: channel > ... > Workflows > "Post to a channel when a webhook request is
# received" (or "Send webhook alerts to a channel"), then copy the URL it shows. The trigger must
# allow "Anyone" to call it; the sig= part of the URL is the secret.
# [[notify]]
# kind = "teams"
# name = "NOC channel"
# url = "https://<env>.environment.api.powerplatform.com/powerautomate/automations/direct/cu/.../invoke?api-version=1&sp=...&sv=1.0&sig=..."
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
        let dec = |b: &[u8]| decode_text(b).map(|d| (d.text, d.encoding));
        assert_eq!(dec(toml.as_bytes()).unwrap(), (toml.to_string(), Encoding::Utf8));
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend_from_slice(toml.as_bytes());
        assert_eq!(dec(&bom8).unwrap(), (toml.to_string(), Encoding::Utf8Bom));
        // What Windows PowerShell 5.1 writes for `--example-config > monitor.toml`.
        let le: Vec<u8> = toml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = toml.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let with = |bom: &[u8], body: &[u8]| [bom, body].concat();
        assert_eq!(dec(&with(&[0xFF, 0xFE], &le)).unwrap(), (toml.to_string(), Encoding::Utf16Le));
        assert_eq!(dec(&with(&[0xFE, 0xFF], &be)).unwrap(), (toml.to_string(), Encoding::Utf16Be));
        assert_eq!(dec(&le).unwrap(), (toml.to_string(), Encoding::Utf16LeNoBom));
        assert_eq!(dec(&be).unwrap(), (toml.to_string(), Encoding::Utf16BeNoBom));
        // A stray second BOM inside the UTF-16 body is dropped.
        let double = with(&[0xFF, 0xFE], &with(&[0xFF, 0xFE], &le));
        assert_eq!(dec(&double).unwrap().0, toml);
        // PowerShell `Set-Content` / old Notepad: ANSI. `é` is 0xE9, the em dash 0x97.
        let ansi = b"# caf\xE9 \x97 x\n[[targets]]\ntarget = \"ping://10.0.0.1\"\n";
        let (text, enc) = dec(ansi).unwrap();
        assert_eq!(enc, Encoding::Windows1252);
        assert!(text.starts_with("# caf\u{E9} \u{2014} x\n"), "{text}");
        assert!(parse_str(&text).is_ok());
        assert_eq!(windows_1252(&[0x81, 0x80]), "\u{FFFD}\u{20AC}");
        assert!(dec(&[0xFF, 0xFE, 0x00, 0x00, 0x41]).unwrap_err().contains("UTF-32"));
        assert!(dec(&[0xFF, 0xFE, 0x41]).unwrap_err().contains("odd number"));
        assert_eq!(utf16_without_bom(b"ab"), None);
        assert_eq!(utf16_without_bom(b"a\0b\0c\0d"), Some(true));
        assert_eq!(utf16_without_bom(b"\0a\0b\0c\0d"), Some(false));
        assert!(!Encoding::Utf8Bom.is_utf8() && Encoding::Utf8.is_utf8());
        assert_eq!(Encoding::Utf16LeNoBom.to_string(), "UTF-16 LE (no byte-order mark)");
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
[[notify]]
kind = "teams"
url = "https://x.environment.api.powerplatform.com:443/powerautomate/automations/direct/cu/1/workflows/2/triggers/manual/paths/invoke?api-version=1&sp=%2Ftriggers%2Fmanual%2Frun&sv=1.0&sig=abc"
[[notify]]
kind = "teams"
url = "http://teams.lan/hook"
"#,
        )
        .unwrap();
        let r = resolve(&cfg).unwrap();
        assert_eq!(r.notifiers[0].tls, TlsClientMode::Insecure);
        assert_eq!(r.notifiers[0].name, "ntfy#1");
        assert_eq!(r.notifiers[1].tls, TlsClientMode::WebPki);
        assert_eq!((r.notifiers[2].kind, r.notifiers[2].tls), (NotifyKind::Teams, TlsClientMode::WebPki));
        assert_eq!(r.notifiers[2].name, "teams#3");
        assert!(r.notifiers[2].url.path.ends_with("&sig=abc"));
        assert_eq!(r.warnings.len(), 4, "{:?}", r.warnings);
        assert!(r.warnings[3].contains("https://"), "{:?}", r.warnings);
        assert!(describe(&r).iter().any(|l| l.contains("teams")));
        assert_eq!(parse_dur("0").unwrap(), Duration::ZERO);
        assert_eq!(parse_dur("1h30m").unwrap(), Duration::from_secs(5400));
        assert!(parse_dur("10").is_err());
    }
}
