//! Statistics engine shared by the client runner, the TUI and the headless summary.
//!
//! Invariants:
//! - `LatencyTracker` is the single source of truth for counters; the UI only renders snapshots.
//! - Percentiles come from an HDR histogram so memory stays constant over multi-hour soaks.

mod latency;
mod ring;
mod snapshot;
mod soak;
mod throughput;

pub use latency::{LatencyTracker, LossPolicy};
pub use ring::RingBuffer;
pub use snapshot::{ChartPoint, LatencySnapshot, RunSummary, Sample, SampleStatus};
pub use soak::{DisconnectEvent, SoakLog, SoakSnapshot};
pub use throughput::{ThroughputSnapshot, ThroughputTracker, TpDirection};
