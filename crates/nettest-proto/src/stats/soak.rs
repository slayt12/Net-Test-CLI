//! Connection-stability bookkeeping: disconnects, reconnect attempts, downtime.
//!
//! Aggregates (counts, total and longest downtime) are kept as running values so a months-long
//! monitor run stays O(1) in memory; only the most recent `max_events` disconnects are retained
//! for the report and the TUI.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Disconnect events kept when no explicit capacity is given (TUI / headless runs).
pub const DEFAULT_EVENT_CAPACITY: usize = 10_000;

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
    events: VecDeque<DisconnectEvent>,
    max_events: usize,
    disconnects: u32,
    reconnect_attempts: u32,
    closed_downtime_ms: u64,
    longest_outage_ms: u64,
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
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }

    /// Keep at most `max_events` disconnect records (aggregates stay exact regardless).
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            started: Instant::now(),
            events: VecDeque::new(),
            max_events: max_events.max(1),
            disconnects: 0,
            reconnect_attempts: 0,
            closed_downtime_ms: 0,
            longest_outage_ms: 0,
            outage_started: None,
            session_started: None,
            longest_session: Duration::ZERO,
            reconnects: 0,
        }
    }

    pub fn on_connected(&mut self, now: Instant) {
        if let Some(outage) = self.outage_started.take() {
            let down = now.duration_since(outage).as_millis() as u64;
            if let Some(last) = self.events.back_mut() {
                last.downtime_ms = Some(down);
            }
            self.closed_downtime_ms += down;
            self.longest_outage_ms = self.longest_outage_ms.max(down);
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
            self.disconnects += 1;
            if self.events.len() == self.max_events {
                self.events.pop_front();
            }
            self.events.push_back(DisconnectEvent {
                at: chrono::Local::now(),
                elapsed_s: (now - self.started).as_secs_f64(),
                reason: reason.into(),
                reconnect_attempts: 0,
                downtime_ms: None,
            });
        }
    }

    pub fn on_reconnect_attempt(&mut self) {
        self.reconnect_attempts += 1;
        if let Some(last) = self.events.back_mut() {
            last.reconnect_attempts += 1;
        }
    }

    pub fn snapshot(&self) -> SoakSnapshot {
        let now = Instant::now();
        let open = self
            .outage_started
            .map(|o| now.duration_since(o).as_millis() as u64)
            .unwrap_or(0);
        let total_down = self.closed_downtime_ms + open;
        let longest = self.longest_outage_ms.max(open);
        let total = now.duration_since(self.started).as_millis().max(1) as f64;
        let current = self
            .session_started
            .map(|s| now.duration_since(s))
            .unwrap_or(Duration::ZERO);
        SoakSnapshot {
            disconnects: self.disconnects,
            reconnects: self.reconnects,
            reconnect_attempts: self.reconnect_attempts,
            total_downtime_ms: total_down,
            longest_outage_ms: longest,
            uptime_pct: ((total - total_down as f64) / total * 100.0).clamp(0.0, 100.0),
            current_session_s: current.as_secs_f64(),
            longest_session_s: self.longest_session.max(current).as_secs_f64(),
            events: self.events.iter().cloned().collect(),
            connected: self.session_started.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_survive_event_eviction() {
        let mut log = SoakLog::with_capacity(2);
        let t0 = Instant::now();
        log.on_connected(t0);
        for i in 0..5u64 {
            let down_at = t0 + Duration::from_secs(10 * i + 1);
            log.on_disconnected(down_at, format!("outage {i}"));
            log.on_reconnect_attempt();
            log.on_connected(down_at + Duration::from_millis(100 * (i + 1)));
        }
        let s = log.snapshot();
        assert_eq!(s.disconnects, 5);
        assert_eq!(s.reconnects, 5);
        assert_eq!(s.reconnect_attempts, 5);
        assert_eq!(s.total_downtime_ms, 100 + 200 + 300 + 400 + 500);
        assert_eq!(s.longest_outage_ms, 500);
        assert_eq!(s.events.len(), 2);
        assert_eq!(s.events[1].reason, "outage 4");
        assert!(s.connected);
    }
}
