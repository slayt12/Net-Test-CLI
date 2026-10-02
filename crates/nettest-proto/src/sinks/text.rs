//! Plain-text sink: same lines as the console, written to a file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::{Event, Sink, ts};
use crate::stats::{LatencySnapshot, Sample};

pub struct TextSink {
    w: BufWriter<File>,
    failures_only: bool,
}

impl TextSink {
    pub fn create(path: &Path, failures_only: bool) -> std::io::Result<Self> {
        Ok(Self {
            w: BufWriter::new(File::create(path)?),
            failures_only,
        })
    }
}

impl Sink for TextSink {
    fn on_sample(&mut self, s: &Sample, snap: &LatencySnapshot) {
        if self.failures_only && !s.status.is_failure() {
            return;
        }
        let rtt = s
            .rtt_ms
            .map(|v| format!("{v:.2} ms"))
            .unwrap_or_else(|| "-".into());
        let _ = writeln!(
            self.w,
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
            self.w,
            "{} {:<5} {}",
            ts(&e.at),
            e.level.as_str(),
            e.describe()
        );
    }

    fn flush(&mut self) {
        let _ = self.w.flush();
    }
}
