//! Bits shared by the echo loop (`latency.rs`) and the serverless loop (`probe.rs`): the drain
//! grace period after the last probe and the summary shape.

use std::time::Duration;

use nettest_proto::stats::{LatencyTracker, LossPolicy, RunSummary, SoakSnapshot};

/// How long a bounded run keeps waiting for replies after its last probe: the loss timeout plus
/// a little slack so the tracker's own expiry fires first.
pub fn policy_timeout(interval: Duration) -> Duration {
    LossPolicy::for_interval(interval).timeout + Duration::from_millis(50)
}

pub fn summary(tracker: &LatencyTracker, soak: Option<SoakSnapshot>) -> RunSummary {
    RunSummary {
        latency: tracker.snapshot(),
        soak,
        ..Default::default()
    }
}
