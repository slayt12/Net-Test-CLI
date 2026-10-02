//! Connection-stability bookkeeping: disconnects, reconnect attempts, downtime.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisconnectEvent {
    pub at: chrono::DateTime<chrono::Local>,
    pub elapsed_s: f64,
    pub reason: String,
    pub reconnect_attempts: u32,
    /// `None` while still disconnected.
    pub downtime_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SoakSnapshot {
    pub disconnects: u32,
    pub reconnects: u32,
    pub reconnect_attempts: u32,
    pub total_downtime_ms: u64,
    pub longest_outage_ms: u64,
    pub uptime_pct: f64,
    pub current_session_s: f64,
    pub longest_session_s: f64,
    pub events: Vec<DisconnectEvent>,
    pub connected: bool,
}

pub struct SoakLog {
    started: Instant,
    events: Vec<DisconnectEvent>,
    outage_started: Option<Instant>,
    session_started: Option<Instant>,
    longest_session: Duration,
    reconnects: u32,
}

impl Default for SoakLog {
    fn default() -> Self {
        Self::new()
    }
}

impl SoakLog {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            events: Vec::new(),
            outage_started: None,
            session_started: None,
            longest_session: Duration::ZERO,
            reconnects: 0,
        }
    }

    pub fn on_connected(&mut self, now: Instant) {
        if let Some(outage) = self.outage_started.take() {
            if let Some(last) = self.events.last_mut() {
                last.downtime_ms = Some(now.duration_since(outage).as_millis() as u64);
            }
            self.reconnects += 1;
        }
        self.session_started = Some(now);
    }

    pub fn on_disconnected(&mut self, now: Instant, reason: impl Into<String>) {
        if let Some(s) = self.session_started.take() {
            self.longest_session = self.longest_session.max(now - s);
        }
        if self.outage_started.is_none() {
            self.outage_started = Some(now);
            self.events.push(DisconnectEvent {
                at: chrono::Local::now(),
                elapsed_s: (now - self.started).as_secs_f64(),
                reason: reason.into(),
                reconnect_attempts: 0,
                downtime_ms: None,
            });
        }
    }

    pub fn on_reconnect_attempt(&mut self) {
        if let Some(last) = self.events.last_mut() {
            last.reconnect_attempts += 1;
        }
    }

    pub fn snapshot(&self) -> SoakSnapshot {
        let now = Instant::now();
        let mut total_down: u64 = self.events.iter().filter_map(|e| e.downtime_ms).sum();
        if let Some(o) = self.outage_started {
            total_down += now.duration_since(o).as_millis() as u64;
        }
        let longest = self
            .events
            .iter()
            .filter_map(|e| e.downtime_ms)
            .max()
            .unwrap_or(0)
            .max(
                self.outage_started
                    .map(|o| now.duration_since(o).as_millis() as u64)
                    .unwrap_or(0),
            );
        let total = now.duration_since(self.started).as_millis().max(1) as f64;
        let current = self
            .session_started
            .map(|s| now.duration_since(s))
            .unwrap_or(Duration::ZERO);
        SoakSnapshot {
            disconnects: self.events.len() as u32,
            reconnects: self.reconnects,
            reconnect_attempts: self.events.iter().map(|e| e.reconnect_attempts).sum(),
            total_downtime_ms: total_down,
            longest_outage_ms: longest,
            uptime_pct: ((total - total_down as f64) / total * 100.0).clamp(0.0, 100.0),
            current_session_s: current.as_secs_f64(),
            longest_session_s: self.longest_session.max(current).as_secs_f64(),
            events: self.events.clone(),
            connected: self.session_started.is_some(),
        }
    }
}
