//! Non-sample events: connects, disconnects, auth, throughput results, errors.

use serde::{Deserialize, Serialize};

use crate::transport::ConnectTimings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    TestStarted {
        mode: String,
        target: String,
    },
    TestStopped {
        reason: String,
    },
    Connected {
        timings: ConnectTimings,
    },
    Disconnected {
        reason: String,
    },
    Reconnecting {
        attempt: u32,
        backoff_ms: u64,
    },
    ThroughputResult {
        direction: String,
        bytes: u64,
        secs: f64,
        mbps: f64,
    },
    Message {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub at: chrono::DateTime<chrono::Local>,
    pub elapsed_s: f64,
    pub level: LogLevel,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn new(level: LogLevel, elapsed_s: f64, kind: EventKind) -> Self {
        Self {
            at: chrono::Local::now(),
            elapsed_s,
            level,
            kind,
        }
    }

    pub fn message(level: LogLevel, elapsed_s: f64, text: impl Into<String>) -> Self {
        Self::new(level, elapsed_s, EventKind::Message { text: text.into() })
    }

    /// One-line human rendering shared by the console sink, text sink and TUI log panel.
    pub fn describe(&self) -> String {
        match &self.kind {
            EventKind::TestStarted { mode, target } => format!("test started: {mode} -> {target}"),
            EventKind::TestStopped { reason } => format!("test stopped: {reason}"),
            EventKind::Connected { timings } => {
                let mut parts = Vec::new();
                if let Some(d) = timings.dns {
                    parts.push(format!("dns {:.1}ms", d.as_secs_f64() * 1e3));
                }
                if let Some(d) = timings.tcp {
                    parts.push(format!("tcp {:.1}ms", d.as_secs_f64() * 1e3));
                }
                if let Some(d) = timings.tls {
                    parts.push(format!("tls {:.1}ms", d.as_secs_f64() * 1e3));
                }
                if let Some(d) = timings.ws_upgrade {
                    parts.push(format!("ws {:.1}ms", d.as_secs_f64() * 1e3));
                }
                if let Some(d) = timings.hello {
                    parts.push(format!("hello {:.1}ms", d.as_secs_f64() * 1e3));
                }
                format!(
                    "connected to {} via {} in {:.1}ms ({})",
                    timings
                        .peer
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "?".into()),
                    timings
                        .protocol
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "?".into()),
                    timings.total.as_secs_f64() * 1e3,
                    parts.join(", ")
                )
            }
            EventKind::Disconnected { reason } => format!("disconnected: {reason}"),
            EventKind::Reconnecting {
                attempt,
                backoff_ms,
            } => {
                format!("reconnecting (attempt {attempt}, waiting {backoff_ms}ms)")
            }
            EventKind::ThroughputResult {
                direction,
                bytes,
                secs,
                mbps,
            } => format!(
                "throughput {direction}: {:.2} MB in {secs:.2}s = {mbps:.2} Mbit/s",
                *bytes as f64 / 1e6
            ),
            EventKind::Message { text } => text.clone(),
        }
    }
}
