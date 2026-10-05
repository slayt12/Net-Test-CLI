//! The test runner: a single tokio task that owns the transport, the trackers and the sinks.
//!
//! The UI (TUI or headless printer) talks to it only through channels, so a slow terminal can
//! never stall probing and the runner never knows whether a human is watching.

mod latency;
mod pacing;
mod probe;
mod reconnect;
mod throughput;

use std::collections::VecDeque;
use std::time::Duration;

use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::sinks::{Event, EventKind, LogLevel, MultiSink, Sink};
use nettest_proto::stats::{
    ChartPoint, LatencySnapshot, RunSummary, Sample, SoakSnapshot, ThroughputSnapshot,
};
use nettest_proto::tls::client::TlsClientMode;
use nettest_proto::transport::{ConnectTimings, DialOptions};
use tokio::sync::mpsc;

pub use reconnect::Backoff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Stop,
}

/// Point-in-time view for the UI. Sent a few times a second.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub latency: LatencySnapshot,
    pub throughput: Option<ThroughputSnapshot>,
    pub soak: Option<SoakSnapshot>,
    pub chart: Vec<ChartPoint>,
    pub connected: bool,
    pub elapsed_s: f64,
}

#[derive(Debug)]
pub enum UiEvent {
    Connected(ConnectTimings),
    Disconnected(String),
    /// `error` is the dial failure for attempts that actually ran; `None` on the scheduling
    /// notice that accompanies `Disconnected`.
    Reconnecting {
        attempt: u32,
        backoff_ms: u64,
        error: Option<String>,
    },
    Sample(Sample),
    Snapshot(Box<Snapshot>),
    Log(Event),
    Finished(Box<RunSummary>),
}

/// Why the runner stopped. Drives the headless exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Completed,
    Stopped,
    ConnectFailed(String),
}

impl StopReason {
    pub fn describe(&self) -> String {
        match self {
            StopReason::Completed => "completed".into(),
            StopReason::Stopped => "stopped by user".into(),
            StopReason::ConnectFailed(e) => format!("connect failed: {e}"),
        }
    }
}

pub struct Runner {
    pub cfg: ClientConfig,
    pub sinks: MultiSink,
    pub events: mpsc::Sender<UiEvent>,
    pub commands: mpsc::Receiver<Command>,
    /// Events and connects kept for the report, bounded by `history_limit` (a months-long
    /// monitor run would otherwise grow without limit).
    pub log: VecDeque<Event>,
    pub connects: VecDeque<ConnectTimings>,
    history_limit: usize,
    started: std::time::Instant,
}

pub struct RunOutput {
    pub summary: RunSummary,
    pub reason: StopReason,
    pub chart: Vec<ChartPoint>,
    pub events: Vec<Event>,
}

impl Runner {
    pub fn new(
        cfg: ClientConfig,
        sinks: MultiSink,
        events: mpsc::Sender<UiEvent>,
        commands: mpsc::Receiver<Command>,
    ) -> Self {
        Self {
            cfg,
            sinks,
            events,
            commands,
            log: VecDeque::new(),
            connects: VecDeque::new(),
            history_limit: usize::MAX,
            started: std::time::Instant::now(),
        }
    }

    /// Cap the retained events / connects (and the soak log's disconnect records). The default
    /// is unlimited so TUI and headless reports are unchanged; the monitor passes 0.
    pub fn set_history_limit(&mut self, n: usize) {
        self.history_limit = n;
    }

    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    fn push_bounded<T>(q: &mut VecDeque<T>, limit: usize, v: T) {
        if limit == 0 {
            return;
        }
        if q.len() >= limit {
            q.pop_front();
        }
        q.push_back(v);
    }

    pub async fn run(mut self) -> RunOutput {
        self.started = std::time::Instant::now();
        let started_at = chrono::Local::now();
        self.emit_log(
            LogLevel::Info,
            EventKind::TestStarted {
                mode: self.cfg.mode.to_string(),
                target: self.cfg.target(),
            },
        );

        let serverless = self.cfg.protocol.is_serverless();
        let (mut summary, reason, chart) = match self.cfg.mode {
            TestMode::Latency | TestMode::Soak if serverless => probe::run(&mut self).await,
            TestMode::Latency | TestMode::Soak => latency::run(&mut self).await,
            TestMode::Throughput if serverless => {
                let why = format!(
                    "throughput needs a nettest-server on the far end; {} is a serverless probe",
                    self.cfg.protocol
                );
                self.emit_message(LogLevel::Error, why.clone());
                (
                    RunSummary::default(),
                    StopReason::ConnectFailed(why),
                    Vec::new(),
                )
            }
            TestMode::Throughput => throughput::run(&mut self).await,
        };
        summary.started_at = Some(started_at);
        summary.finished_at = Some(chrono::Local::now());
        summary.host = self.cfg.host.clone();
        summary.port = self.cfg.port;
        summary.protocol = Some(self.cfg.protocol);
        summary.mode = self.cfg.mode.to_string();
        summary.interval_ms = self.cfg.interval_ms;
        summary.payload_bytes = self.cfg.payload_bytes;
        summary.connects = self.connects.iter().cloned().collect();
        summary.stop_reason = reason.describe();

        self.emit_log(
            LogLevel::Info,
            EventKind::TestStopped {
                reason: reason.describe(),
            },
        );
        self.sinks.flush();
        let _ = self
            .events
            .send(UiEvent::Finished(Box::new(summary.clone())))
            .await;
        RunOutput {
            summary,
            reason,
            chart,
            events: self.log.into_iter().collect(),
        }
    }

    pub fn elapsed_s(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn dial_options(&self) -> Result<DialOptions, String> {
        let tls = if self.cfg.protocol.is_tls() {
            if !self.cfg.fingerprint.trim().is_empty() {
                let fp = nettest_proto::tls::fingerprint::parse(&self.cfg.fingerprint)?;
                Some(TlsClientMode::Pinned(fp))
            } else if self.cfg.insecure {
                Some(TlsClientMode::Insecure)
            } else {
                return Err("wss requires --insecure or --fingerprint".into());
            }
        } else {
            None
        };
        Ok(DialOptions {
            host: self.cfg.host.clone(),
            port: self.cfg.port,
            protocol: self.cfg.protocol,
            tls,
            token: if self.cfg.token.is_empty() {
                None
            } else {
                Some(self.cfg.token.clone())
            },
            client_id: client_id(),
            mode: self.cfg.mode.to_string(),
            connect_timeout: Duration::from_secs(self.cfg.connect_timeout_secs.max(1)),
            prefer_ipv6: self.cfg.ipv6,
            bind: None,
            ws_path: self.cfg.ws_path.clone(),
        })
    }

    pub fn emit_log(&mut self, level: LogLevel, kind: EventKind) {
        let ev = Event::new(level, self.elapsed_s(), kind);
        self.sinks.on_event(&ev);
        Self::push_bounded(&mut self.log, self.history_limit, ev.clone());
        let _ = self.events.try_send(UiEvent::Log(ev));
    }

    pub fn emit_message(&mut self, level: LogLevel, text: impl Into<String>) {
        self.emit_log(level, EventKind::Message { text: text.into() });
    }

    pub fn on_connected(&mut self, t: &ConnectTimings) {
        Self::push_bounded(&mut self.connects, self.history_limit, t.clone());
        self.emit_log(LogLevel::Info, EventKind::Connected { timings: t.clone() });
        let _ = self.events.try_send(UiEvent::Connected(t.clone()));
    }

    pub fn on_disconnected(&mut self, reason: &str) {
        self.emit_log(
            LogLevel::Warn,
            EventKind::Disconnected {
                reason: reason.to_string(),
            },
        );
        let _ = self
            .events
            .try_send(UiEvent::Disconnected(reason.to_string()));
    }

    pub fn on_sample(&mut self, s: &Sample, snap: &LatencySnapshot) {
        self.sinks.on_sample(s, snap);
        let _ = self.events.try_send(UiEvent::Sample(s.clone()));
    }

    /// Non-blocking check for a Stop command.
    pub fn stop_requested(&mut self) -> bool {
        matches!(self.commands.try_recv(), Ok(Command::Stop))
    }
}

/// Stable-ish identifier shown in the server's client table.
pub fn client_id() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "client".into());
    format!("{host}#{}", std::process::id())
}

/// Deterministic pseudo-random filler so payloads are not trivially compressible by middleboxes.
pub fn filler(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut v = Vec::with_capacity(len);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(len);
    v
}
