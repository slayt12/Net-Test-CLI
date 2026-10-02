//! Plain-data views of the trackers. Everything here is `Clone + Serialize` so it can cross the
//! runner -> UI channel and be written by sinks without touching the trackers.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::transport::{ConnectTimings, Protocol};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleStatus {
    Ok,
    Lost,
    /// Arrived after being declared lost.
    Late,
    Duplicate,
    OutOfOrder,
    /// Definitive negative answer before the timeout (connection refused, ICMP unreachable).
    /// Counted as lost for loss %, kept separate so the report can say why.
    Failed,
}

impl SampleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SampleStatus::Ok => "ok",
            SampleStatus::Lost => "lost",
            SampleStatus::Late => "late",
            SampleStatus::Duplicate => "dup",
            SampleStatus::OutOfOrder => "ooo",
            SampleStatus::Failed => "fail",
        }
    }
    pub fn is_failure(self) -> bool {
        !matches!(self, SampleStatus::Ok)
    }
}

/// One probe outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub seq: u64,
    /// Wall-clock time the probe was sent.
    pub sent_at: chrono::DateTime<chrono::Local>,
    /// Seconds since the test started (chart X axis).
    pub elapsed_s: f64,
    pub rtt_ms: Option<f64>,
    pub status: SampleStatus,
    /// Free text from serverless probes: the SIP/HTTP status line or the failure reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Chart point: `rtt_ms == None` marks a loss at that moment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChartPoint {
    pub t: f64,
    pub rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencySnapshot {
    pub sent: u64,
    pub received: u64,
    pub lost: u64,
    pub late: u64,
    pub duplicates: u64,
    pub out_of_order: u64,
    /// Subset of `lost` that failed definitively (refused / unreachable) instead of timing out.
    #[serde(default)]
    pub failed: u64,
    pub in_flight: u64,
    pub loss_pct: f64,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub jitter_ms: f64,
    pub last_ms: Option<f64>,
    pub elapsed: Duration,
}

/// Everything needed to print a final summary or build the HTML report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunSummary {
    pub started_at: Option<chrono::DateTime<chrono::Local>>,
    pub finished_at: Option<chrono::DateTime<chrono::Local>>,
    pub host: String,
    pub port: u16,
    pub protocol: Option<Protocol>,
    pub mode: String,
    pub interval_ms: u64,
    pub payload_bytes: u32,
    pub latency: LatencySnapshot,
    pub throughput: Option<super::ThroughputSnapshot>,
    pub soak: Option<super::SoakSnapshot>,
    pub connects: Vec<ConnectTimings>,
    pub stop_reason: String,
}
