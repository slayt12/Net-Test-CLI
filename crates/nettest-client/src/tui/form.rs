//! Settings form model: an ordered list of fields over `ClientConfig`, each either free text,
//! a toggle or a cycling enum. Text edits go through a buffer and are validated on commit so the
//! config can never hold an unparsable value.

use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::transport::Protocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldId {
    Host,
    Port,
    Protocol,
    Mode,
    Interval,
    Size,
    Duration,
    Count,
    Token,
    Insecure,
    Fingerprint,
    Heartbeat,
    OutageAfter,
    TpSecs,
    TpChunk,
    TpDirection,
    SinkCsv,
    SinkText,
    SinkJsonl,
    SinkHtml,
    FailuresOnly,
    OutputDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Toggle,
    Cycle,
    /// Section title; not selectable.
    Header,
    /// Read-only line; not selectable.
    Info,
}

impl FieldKind {
    pub fn selectable(self) -> bool {
        !matches!(self, FieldKind::Header | FieldKind::Info)
    }
}

pub struct FieldDef {
    pub id: FieldId,
    pub label: &'static str,
    pub kind: FieldKind,
    pub hint: &'static str,
}

pub const FIELDS: &[FieldDef] = &[
    FieldDef {
        id: FieldId::Host,
        label: "Target host",
        kind: FieldKind::Text,
        hint: "nettest-server, or any device for ping / connect / sip / http",
    },
    FieldDef {
        id: FieldId::Port,
        label: "Port",
        kind: FieldKind::Text,
        hint: "1-65535",
    },
    FieldDef {
        id: FieldId::Protocol,
        label: "Protocol",
        kind: FieldKind::Cycle,
        hint: "ws / wss / tcp / udp need nettest-server; ping / connect / sip / http / https work on any device",
    },
    FieldDef {
        id: FieldId::Mode,
        label: "Test mode",
        kind: FieldKind::Cycle,
        hint: "latency (probes) / soak (stability) / throughput (nettest-server only)",
    },
    FieldDef {
        id: FieldId::Interval,
        label: "Probe interval (ms)",
        kind: FieldKind::Text,
        hint: "latency mode; 1 ms minimum",
    },
    FieldDef {
        id: FieldId::Size,
        label: "Frame size (bytes)",
        kind: FieldKind::Text,
        hint: "whole frame incl. 32-byte header; UDP max 1400; ping: ICMP message size",
    },
    FieldDef {
        id: FieldId::Duration,
        label: "Duration (s, 0=unlimited)",
        kind: FieldKind::Text,
        hint: "stop after this many seconds",
    },
    FieldDef {
        id: FieldId::Count,
        label: "Probe count (0=unlimited)",
        kind: FieldKind::Text,
        hint: "stop after this many probes",
    },
    FieldDef {
        id: FieldId::Token,
        label: "Auth token",
        kind: FieldKind::Text,
        hint: "must match the server's --token",
    },
    FieldDef {
        id: FieldId::Insecure,
        label: "TLS: accept any cert",
        kind: FieldKind::Toggle,
        hint: "wss only; otherwise pin a fingerprint below",
    },
    FieldDef {
        id: FieldId::Fingerprint,
        label: "TLS: pinned SHA-256",
        kind: FieldKind::Text,
        hint: "paste from the server log (colons optional)",
    },
    FieldDef {
        id: FieldId::Heartbeat,
        label: "Heartbeat (ms)",
        kind: FieldKind::Text,
        hint: "soak mode cadence",
    },
    FieldDef {
        id: FieldId::OutageAfter,
        label: "Outage after N failures",
        kind: FieldKind::Text,
        hint: "serverless soak: consecutive failed probes that open an outage",
    },
    FieldDef {
        id: FieldId::TpSecs,
        label: "Throughput seconds",
        kind: FieldKind::Text,
        hint: "per direction",
    },
    FieldDef {
        id: FieldId::TpChunk,
        label: "Throughput chunk (bytes)",
        kind: FieldKind::Text,
        hint: "bytes per frame (UDP capped at 1368)",
    },
    FieldDef {
        id: FieldId::TpDirection,
        label: "Throughput direction",
        kind: FieldKind::Cycle,
        hint: "upload / download / both",
    },
    FieldDef {
        id: FieldId::SinkCsv,
        label: "Log: CSV file",
        kind: FieldKind::Toggle,
        hint: "nettest-<timestamp>.csv in output dir",
    },
    FieldDef {
        id: FieldId::SinkText,
        label: "Log: text file",
        kind: FieldKind::Toggle,
        hint: "nettest-<timestamp>.log",
    },
    FieldDef {
        id: FieldId::SinkJsonl,
        label: "Log: JSON lines",
        kind: FieldKind::Toggle,
        hint: "nettest-<timestamp>.jsonl",
    },
    FieldDef {
        id: FieldId::SinkHtml,
        label: "HTML report at end",
        kind: FieldKind::Toggle,
        hint: "nettest-<timestamp>.html, also 'r' while running",
    },
    FieldDef {
        id: FieldId::FailuresOnly,
        label: "Log failures only",
        kind: FieldKind::Toggle,
        hint: "off = every probe goes to file logs / panel",
    },
    FieldDef {
        id: FieldId::OutputDir,
        label: "Output directory",
        kind: FieldKind::Text,
        hint: "created if missing",
    },
];

pub fn value_of(cfg: &ClientConfig, id: FieldId) -> String {
    match id {
        FieldId::Host => cfg.host.clone(),
        FieldId::Port => cfg.port.to_string(),
        FieldId::Protocol => cfg.protocol.to_string(),
        FieldId::Mode => cfg.mode.to_string(),
        FieldId::Interval => cfg.interval_ms.to_string(),
        FieldId::Size => cfg.payload_bytes.to_string(),
        FieldId::Duration => cfg.duration_secs.to_string(),
        FieldId::Count => cfg.count.to_string(),
        FieldId::Token => cfg.token.clone(),
        FieldId::Insecure => on_off(cfg.insecure),
        FieldId::Fingerprint => cfg.fingerprint.clone(),
        FieldId::Heartbeat => cfg.heartbeat_ms.to_string(),
        FieldId::OutageAfter => cfg.outage_after.to_string(),
        FieldId::TpSecs => cfg.tp_secs.to_string(),
        FieldId::TpChunk => cfg.tp_chunk_bytes.to_string(),
        FieldId::TpDirection => cfg.tp_direction.to_string(),
        FieldId::SinkCsv => on_off(cfg.sinks.csv),
        FieldId::SinkText => on_off(cfg.sinks.text),
        FieldId::SinkJsonl => on_off(cfg.sinks.jsonl),
        FieldId::SinkHtml => on_off(cfg.sinks.html_report),
        FieldId::FailuresOnly => on_off(cfg.sinks.console_failures_only),
        FieldId::OutputDir => cfg.sinks.output_dir.clone(),
    }
}

fn on_off(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

/// Space / Enter on a toggle or cycle field.
pub fn cycle(cfg: &mut ClientConfig, id: FieldId) {
    match id {
        FieldId::Protocol => cfg.protocol = cfg.protocol.next(),
        FieldId::Mode => cfg.mode = cfg.mode.next(),
        FieldId::TpDirection => cfg.tp_direction = cfg.tp_direction.next(),
        FieldId::Insecure => cfg.insecure = !cfg.insecure,
        FieldId::SinkCsv => cfg.sinks.csv = !cfg.sinks.csv,
        FieldId::SinkText => cfg.sinks.text = !cfg.sinks.text,
        FieldId::SinkJsonl => cfg.sinks.jsonl = !cfg.sinks.jsonl,
        FieldId::SinkHtml => cfg.sinks.html_report = !cfg.sinks.html_report,
        FieldId::FailuresOnly => cfg.sinks.console_failures_only = !cfg.sinks.console_failures_only,
        _ => {}
    }
}

/// Commit an edited text value. Returns a user-facing error on invalid input.
pub fn commit(cfg: &mut ClientConfig, id: FieldId, text: &str) -> Result<(), String> {
    let t = text.trim();
    let num = |what: &str| -> Result<u64, String> {
        t.parse::<u64>()
            .map_err(|_| format!("{what} must be a whole number"))
    };
    match id {
        FieldId::Host => {
            if t.is_empty() {
                return Err("host cannot be empty".into());
            }
            cfg.host = t.trim_matches(['[', ']']).to_string();
        }
        FieldId::Port => {
            let p = num("port")?;
            if !(1..=65535).contains(&p) {
                return Err("port must be 1-65535".into());
            }
            cfg.port = p as u16;
        }
        FieldId::Interval => cfg.interval_ms = num("interval")?.max(1),
        FieldId::Size => {
            let s = num("size")?;
            if s < nettest_proto::frame::HEADER_LEN as u64 {
                return Err("size must be at least 32 bytes".into());
            }
            if cfg.protocol == Protocol::Ping && s > 65507 {
                return Err("ICMP messages cannot exceed 65507 bytes".into());
            }
            if cfg.protocol == Protocol::Udp && s > 1400 {
                return Err(
                    "UDP frames larger than 1400 bytes will fragment; lower the size".into(),
                );
            }
            cfg.payload_bytes = s.min(u32::MAX as u64) as u32;
        }
        FieldId::Duration => cfg.duration_secs = num("duration")?,
        FieldId::Count => cfg.count = num("count")?,
        FieldId::Token => cfg.token = t.to_string(),
        FieldId::Fingerprint => {
            if !t.is_empty() {
                nettest_proto::tls::fingerprint::parse(t)?;
            }
            cfg.fingerprint = t.to_string();
        }
        FieldId::Heartbeat => cfg.heartbeat_ms = num("heartbeat")?.max(100),
        FieldId::OutageAfter => cfg.outage_after = num("outage threshold")?.clamp(1, 1000) as u32,
        FieldId::TpSecs => cfg.tp_secs = num("throughput seconds")?.clamp(1, 3600),
        FieldId::TpChunk => cfg.tp_chunk_bytes = num("chunk")?.clamp(1, 16 * 1024 * 1024) as u32,
        FieldId::OutputDir => {
            cfg.sinks.output_dir = if t.is_empty() {
                ".".into()
            } else {
                t.to_string()
            }
        }
        _ => {}
    }
    Ok(())
}

/// Problems that would make Start fail; shown before launching.
pub fn preflight(cfg: &ClientConfig) -> Result<(), String> {
    if cfg.host.trim().is_empty() {
        return Err("host is required".into());
    }
    if cfg.protocol.is_tls() && !cfg.insecure && cfg.fingerprint.trim().is_empty() {
        return Err("wss needs 'accept any cert' on, or a pinned fingerprint".into());
    }
    if cfg.mode == TestMode::Throughput && cfg.protocol.is_serverless() {
        return Err(format!(
            "throughput needs a nettest-server; {} works with latency or soak",
            cfg.protocol
        ));
    }
    if cfg.protocol == Protocol::Connect && cfg.port == 0 {
        return Err("connect needs the port to test, e.g. 5060".into());
    }
    if !cfg.fingerprint.trim().is_empty() && relevant(cfg, FieldId::Fingerprint) {
        nettest_proto::tls::fingerprint::parse(&cfg.fingerprint)?;
    }
    Ok(())
}

/// Whether a field has any effect with the current protocol/mode. Irrelevant fields stay in the
/// list (so cursor positions are stable) but are drawn dimmed.
pub fn relevant(cfg: &ClientConfig, id: FieldId) -> bool {
    let p = cfg.protocol;
    let serverless = p.is_serverless();
    match id {
        FieldId::Port => p != Protocol::Ping,
        FieldId::Size => !serverless || p == Protocol::Ping,
        FieldId::Token => !serverless,
        FieldId::Insecure => p == Protocol::Wss,
        FieldId::Fingerprint => matches!(p, Protocol::Wss | Protocol::Https),
        FieldId::Interval => cfg.mode != TestMode::Throughput,
        FieldId::Heartbeat => cfg.mode == TestMode::Soak,
        FieldId::OutageAfter => serverless && cfg.mode == TestMode::Soak,
        FieldId::TpSecs | FieldId::TpChunk | FieldId::TpDirection => {
            !serverless && cfg.mode == TestMode::Throughput
        }
        FieldId::Duration | FieldId::Count => cfg.mode != TestMode::Throughput,
        _ => true,
    }
}
