//! JSON Lines sink: one object per sample/event, for ingestion by log tooling.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::Serialize;

use super::{Event, Sink};
use crate::stats::{LatencySnapshot, Sample};

pub struct JsonlSink {
    w: BufWriter<File>,
}

#[derive(Serialize)]
struct SampleLine<'a> {
    kind: &'static str,
    #[serde(flatten)]
    sample: &'a Sample,
    loss_pct: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    jitter_ms: f64,
}

#[derive(Serialize)]
struct EventLine<'a> {
    kind: &'static str,
    #[serde(flatten)]
    event: &'a Event,
}

impl JsonlSink {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            w: BufWriter::new(File::create(path)?),
        })
    }
}

impl Sink for JsonlSink {
    fn on_sample(&mut self, s: &Sample, snap: &LatencySnapshot) {
        let line = SampleLine {
            kind: "sample",
            sample: s,
            loss_pct: snap.loss_pct,
            p50_ms: snap.p50_ms,
            p95_ms: snap.p95_ms,
            p99_ms: snap.p99_ms,
            jitter_ms: snap.jitter_ms,
        };
        if serde_json::to_writer(&mut self.w, &line).is_ok() {
            let _ = self.w.write_all(b"\n");
        }
    }

    fn on_event(&mut self, e: &Event) {
        let line = EventLine {
            kind: "event",
            event: e,
        };
        if serde_json::to_writer(&mut self.w, &line).is_ok() {
            let _ = self.w.write_all(b"\n");
        }
    }

    fn flush(&mut self) {
        let _ = self.w.flush();
    }
}
