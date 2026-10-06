//! Client settings: everything the TUI form edits and the CLI flags override.

use serde::{Deserialize, Serialize};

use crate::stats::TpDirection;
use crate::transport::Protocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum TestMode {
    #[default]
    Latency,
    Soak,
    Throughput,
}

impl TestMode {
    pub const ALL: [TestMode; 3] = [TestMode::Latency, TestMode::Soak, TestMode::Throughput];
    pub fn as_str(self) -> &'static str {
        match self {
            TestMode::Latency => "latency",
            TestMode::Soak => "soak",
            TestMode::Throughput => "throughput",
        }
    }
    pub fn next(self) -> Self {
        let i = TestMode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        TestMode::ALL[(i + 1) % TestMode::ALL.len()]
    }
}

impl std::fmt::Display for TestMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SinkConfig {
    pub console: bool,
    pub console_failures_only: bool,
    pub csv: bool,
    pub text: bool,
    pub jsonl: bool,
    pub html_report: bool,
    pub output_dir: String,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            console: true,
            console_failures_only: true,
            csv: false,
            text: false,
            jsonl: false,
            html_report: true,
            output_dir: ".".into(),
        }
    }
}

/// Webhook alerts for interactive / headless runs. The notifiers themselves live in
/// `monitor.toml` (`[[notify]]`, shared with the monitor service) so a webhook is configured
/// once; these are only the thresholds and switches that apply to a TUI run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertConfig {
    /// Master switch; with no `[[notify]]` entries nothing is sent anyway.
    pub enabled: bool,
    /// Send "test started" when a run begins.
    pub on_start: bool,
    /// Send a summary when a run ends.
    pub on_finish: bool,
    /// Consecutive failures that open an outage alert (DOWN).
    pub failures_before_down: u32,
    /// Consecutive successes that close it (UP).
    pub successes_before_up: u32,
    /// Re-send the DOWN alert at this cadence while still down; "0" = never.
    pub remind_every: String,
    /// Name of this machine in alert text ("" = detect).
    pub hostname: String,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_start: false,
            on_finish: false,
            failures_before_down: 3,
            successes_before_up: 1,
            remind_every: "0".into(),
            hostname: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClientConfig {
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    pub mode: TestMode,
    pub interval_ms: u64,
    /// Total frame size on the wire (header included).
    pub payload_bytes: u32,
    /// 0 = unlimited.
    pub duration_secs: u64,
    /// 0 = unlimited.
    pub count: u64,
    pub token: String,
    pub insecure: bool,
    pub fingerprint: String,
    pub connect_timeout_secs: u64,
    pub heartbeat_ms: u64,
    pub tp_secs: u64,
    pub tp_chunk_bytes: u32,
    pub tp_direction: TpDirection,
    pub ipv6: Option<bool>,
    /// WebSocket request path, or the HTTP(S) probe path.
    pub ws_path: String,
    /// Serverless soak: consecutive failed probes that open an outage.
    pub outage_after: u32,
    /// Loss timeout per probe in ms; 0 = automatic (`max(2 s, 10 × interval)`). The monitor sets
    /// it so a slow cadence still declares a probe lost quickly.
    pub loss_timeout_ms: u64,
    pub sinks: SinkConfig,
    pub alerts: AlertConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 9100,
            protocol: Protocol::Ws,
            mode: TestMode::Latency,
            interval_ms: 1000,
            payload_bytes: 64,
            duration_secs: 0,
            count: 0,
            token: String::new(),
            insecure: false,
            fingerprint: String::new(),
            connect_timeout_secs: 5,
            heartbeat_ms: 5000,
            tp_secs: 10,
            tp_chunk_bytes: 64 * 1024,
            tp_direction: TpDirection::Both,
            ipv6: None,
            ws_path: "/".into(),
            outage_after: 3,
            loss_timeout_ms: 0,
            sinks: SinkConfig::default(),
            alerts: AlertConfig::default(),
        }
    }
}

impl ClientConfig {
    pub fn target(&self) -> String {
        if self.protocol == Protocol::Ping {
            format!("ping://{}", self.host)
        } else {
            format!("{}://{}:{}", self.protocol, self.host, self.port)
        }
    }
}
