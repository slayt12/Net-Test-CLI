//! Log sinks. The runner owns a `MultiSink` and fans every sample and event out to all enabled
//! sinks; the UI never writes files, so a stalled terminal can never drop log lines.
//!
//! Sinks are synchronous `BufWriter`s: at probe rates up to ~1 kHz the write cost is noise and
//! it keeps the runner free of extra tasks.

mod console;
mod csv_sink;
mod event;
mod jsonl;
mod text;

pub use console::ConsoleSink;
pub use csv_sink::CsvSink;
pub use event::{Event, EventKind, LogLevel};
pub use jsonl::JsonlSink;
pub use text::TextSink;

use crate::stats::{LatencySnapshot, Sample};

pub trait Sink: Send {
    fn on_sample(&mut self, sample: &Sample, snap: &LatencySnapshot);
    fn on_event(&mut self, event: &Event);
    fn flush(&mut self);
}

#[derive(Default)]
pub struct MultiSink {
    sinks: Vec<Box<dyn Sink>>,
}

impl MultiSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, s: Box<dyn Sink>) {
        self.sinks.push(s);
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl Sink for MultiSink {
    fn on_sample(&mut self, sample: &Sample, snap: &LatencySnapshot) {
        for s in &mut self.sinks {
            s.on_sample(sample, snap);
        }
    }
    fn on_event(&mut self, event: &Event) {
        for s in &mut self.sinks {
            s.on_event(event);
        }
    }
    fn flush(&mut self) {
        for s in &mut self.sinks {
            s.flush();
        }
    }
}

/// Timestamp format used by every text-ish sink so lines from different files line up.
pub fn ts(dt: &chrono::DateTime<chrono::Local>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}
