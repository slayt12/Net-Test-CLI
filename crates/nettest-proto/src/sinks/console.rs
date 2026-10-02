//! Console sink: one line per sample (or only failures) plus every event, to stderr so stdout
//! stays clean for the machine-readable summary in headless mode.

use std::io::Write;

use super::{Event, Sink, ts};
use crate::stats::{LatencySnapshot, Sample};

pub struct ConsoleSink {
    failures_only: bool,
    out: std::io::Stderr,
}

impl ConsoleSink {
    pub fn new(failures_only: bool) -> Self {
        Self {
            failures_only,
            out: std::io::stderr(),
        }
    }
}

impl Sink for ConsoleSink {
    fn on_sample(&mut self, s: &Sample, snap: &LatencySnapshot) {
        if self.failures_only && !s.status.is_failure() {
            return;
        }
        let rtt = s
            .rtt_ms
            .map(|v| format!("{v:.2} ms"))
            .unwrap_or_else(|| "-".into());
        let _ = writeln!(
            self.out,
            "{} seq={:<7} {:<5} rtt={:<10} loss={:.2}% p95={:.2}ms jitter={:.2}ms{}",
            ts(&s.sent_at),
            s.seq,
            s.status.as_str(),
            rtt,
            snap.loss_pct,
            snap.p95_ms,
            snap.jitter_ms,
            s.detail
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        );
    }

    fn on_event(&mut self, e: &Event) {
        let _ = writeln!(
            self.out,
            "{} {:<5} {}",
            ts(&e.at),
            e.level.as_str(),
            e.describe()
        );
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}
