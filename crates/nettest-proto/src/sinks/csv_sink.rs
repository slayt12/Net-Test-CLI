//! CSV sink: one row per sample; events go in the same file with `kind=event` so a single file
//! tells the whole story when imported into a spreadsheet.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use super::{Event, Sink, ts};
use crate::stats::{LatencySnapshot, Sample};

pub struct CsvSink {
    w: csv::Writer<BufWriter<File>>,
}

impl CsvSink {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let f = File::create(path)?;
        let mut w = csv::Writer::from_writer(BufWriter::new(f));
        w.write_record([
            "timestamp",
            "elapsed_s",
            "kind",
            "seq",
            "status",
            "rtt_ms",
            "loss_pct",
            "p50_ms",
            "p95_ms",
            "p99_ms",
            "jitter_ms",
            "detail",
        ])?;
        Ok(Self { w })
    }
}

impl Sink for CsvSink {
    fn on_sample(&mut self, s: &Sample, snap: &LatencySnapshot) {
        let _ = self.w.write_record([
            ts(&s.sent_at),
            format!("{:.3}", s.elapsed_s),
            "sample".into(),
            s.seq.to_string(),
            s.status.as_str().into(),
            s.rtt_ms.map(|v| format!("{v:.3}")).unwrap_or_default(),
            format!("{:.3}", snap.loss_pct),
            format!("{:.3}", snap.p50_ms),
            format!("{:.3}", snap.p95_ms),
            format!("{:.3}", snap.p99_ms),
            format!("{:.3}", snap.jitter_ms),
            s.detail.clone().unwrap_or_default(),
        ]);
    }

    fn on_event(&mut self, e: &Event) {
        let _ = self.w.write_record([
            ts(&e.at),
            format!("{:.3}", e.elapsed_s),
            "event".into(),
            String::new(),
            e.level.as_str().into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            e.describe(),
        ]);
    }

    fn flush(&mut self) {
        let _ = self.w.flush();
    }
}
