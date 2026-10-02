//! Byte counters for the throughput mode with a one-second sliding "instant" rate.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum, Default)]
#[serde(rename_all = "lowercase")]
pub enum TpDirection {
    #[default]
    Upload,
    Download,
    Both,
}

impl TpDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            TpDirection::Upload => "upload",
            TpDirection::Download => "download",
            TpDirection::Both => "both",
        }
    }
    pub fn next(self) -> Self {
        match self {
            TpDirection::Upload => TpDirection::Download,
            TpDirection::Download => TpDirection::Both,
            TpDirection::Both => TpDirection::Upload,
        }
    }
}

impl std::fmt::Display for TpDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThroughputSnapshot {
    pub up_bytes: u64,
    pub up_secs: f64,
    pub up_mbps: f64,
    pub up_frames: u64,
    pub down_bytes: u64,
    pub down_secs: f64,
    pub down_mbps: f64,
    pub down_frames: u64,
    /// Datagrams/frames the sender said it transmitted (from `TpEnd.seq`); 0 if unknown.
    pub down_expected_frames: u64,
    pub down_loss_pct: f64,
    pub instant_mbps: f64,
}

pub struct ThroughputTracker {
    pub up: Counter,
    pub down: Counter,
    window_start: Instant,
    window_bytes: u64,
    instant_mbps: f64,
}

#[derive(Debug, Default, Clone)]
pub struct Counter {
    pub bytes: u64,
    pub frames: u64,
    pub expected_frames: u64,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
}

impl Counter {
    pub fn secs(&self) -> f64 {
        match (self.started, self.finished) {
            (Some(s), Some(f)) => (f - s).as_secs_f64(),
            (Some(s), None) => s.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }
    pub fn mbps(&self) -> f64 {
        mbps(self.bytes, self.secs())
    }
}

pub fn mbps(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        0.0
    } else {
        bytes as f64 * 8.0 / secs / 1_000_000.0
    }
}

impl Default for ThroughputTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ThroughputTracker {
    pub fn new() -> Self {
        Self {
            up: Counter::default(),
            down: Counter::default(),
            window_start: Instant::now(),
            window_bytes: 0,
            instant_mbps: 0.0,
        }
    }

    pub fn start(&mut self, dir: TpDirection, now: Instant) {
        let c = self.counter_mut(dir);
        c.started = Some(now);
        c.finished = None;
        self.window_start = now;
        self.window_bytes = 0;
    }

    pub fn add(&mut self, dir: TpDirection, bytes: u64, now: Instant) {
        let c = self.counter_mut(dir);
        c.bytes += bytes;
        c.frames += 1;
        self.window_bytes += bytes;
        let w = now.duration_since(self.window_start);
        if w >= Duration::from_secs(1) {
            self.instant_mbps = mbps(self.window_bytes, w.as_secs_f64());
            self.window_start = now;
            self.window_bytes = 0;
        }
    }

    pub fn finish(&mut self, dir: TpDirection, now: Instant, expected_frames: u64) {
        let c = self.counter_mut(dir);
        c.finished = Some(now);
        c.expected_frames = expected_frames;
    }

    /// Server-reported totals override local upload counts: only the receiver knows what landed.
    pub fn set_upload_result(&mut self, bytes: u64, elapsed: Duration, frames: u64) {
        self.up.bytes = bytes;
        self.up.frames = frames;
        let s = self.up.started.unwrap_or_else(Instant::now);
        self.up.started = Some(s);
        self.up.finished = Some(s + elapsed);
    }

    fn counter_mut(&mut self, dir: TpDirection) -> &mut Counter {
        match dir {
            TpDirection::Download => &mut self.down,
            _ => &mut self.up,
        }
    }

    pub fn snapshot(&self) -> ThroughputSnapshot {
        ThroughputSnapshot {
            up_bytes: self.up.bytes,
            up_secs: self.up.secs(),
            up_mbps: self.up.mbps(),
            up_frames: self.up.frames,
            down_bytes: self.down.bytes,
            down_secs: self.down.secs(),
            down_mbps: self.down.mbps(),
            down_frames: self.down.frames,
            down_expected_frames: self.down.expected_frames,
            down_loss_pct: if self.down.expected_frames > 0 {
                ((self.down.expected_frames.saturating_sub(self.down.frames)) as f64
                    / self.down.expected_frames as f64
                    * 100.0)
                    .clamp(0.0, 100.0)
            } else {
                0.0
            },
            instant_mbps: self.instant_mbps,
        }
    }
}
