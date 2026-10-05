//! Per-target health state machine: consecutive-failure counting, pure and clock-free so it is
//! unit tested exhaustively.
//!
//! Rules (agreed with the user): DOWN after `down_after` consecutive failures, UP after
//! `up_after` consecutive successes, an optional reminder while DOWN. The first verdict after
//! start is reported as `Initial` so a target that is already down when the service starts
//! still produces a DOWN alert after `down_after` failures, but a healthy one is not announced
//! as "UP" out of nowhere.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub down_after: u32,
    pub up_after: u32,
    pub remind_every: Option<Duration>,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            down_after: 3,
            up_after: 1,
            remind_every: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Success,
    Failure(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Unknown,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// First verdict after start; logged, never alerted when `up`.
    Initial { up: bool, reason: String },
    Down { reason: String, consecutive: u32 },
    Up { downtime: Duration, successes: u32 },
    Remind { downtime: Duration, reason: String },
}

#[derive(Debug)]
pub struct Health {
    t: Thresholds,
    state: State,
    fails: u32,
    oks: u32,
    down_since: Option<Instant>,
    last_alert: Option<Instant>,
    last_reason: String,
}

impl Health {
    pub fn new(t: Thresholds) -> Self {
        Self {
            t: Thresholds {
                down_after: t.down_after.max(1),
                up_after: t.up_after.max(1),
                remind_every: t.remind_every.filter(|d| !d.is_zero()),
            },
            state: State::Unknown,
            fails: 0,
            oks: 0,
            down_since: None,
            last_alert: None,
            last_reason: String::new(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn down_since(&self) -> Option<Instant> {
        self.down_since
    }

    pub fn last_reason(&self) -> &str {
        &self.last_reason
    }

    pub fn on_signal(&mut self, s: Signal, now: Instant) -> Option<Transition> {
        match s {
            Signal::Success => {
                self.fails = 0;
                self.oks += 1;
                match self.state {
                    State::Unknown => {
                        self.state = State::Up;
                        Some(Transition::Initial {
                            up: true,
                            reason: String::new(),
                        })
                    }
                    State::Down if self.oks >= self.t.up_after => {
                        self.state = State::Up;
                        let downtime = self
                            .down_since
                            .take()
                            .map(|d| now.duration_since(d))
                            .unwrap_or_default();
                        self.last_alert = None;
                        Some(Transition::Up {
                            downtime,
                            successes: self.oks,
                        })
                    }
                    _ => None,
                }
            }
            Signal::Failure(reason) => {
                self.oks = 0;
                self.fails += 1;
                self.last_reason = reason.clone();
                match self.state {
                    State::Up | State::Unknown if self.fails >= self.t.down_after => {
                        let first = self.state == State::Unknown;
                        self.state = State::Down;
                        self.down_since = Some(now);
                        self.last_alert = Some(now);
                        if first {
                            Some(Transition::Initial { up: false, reason })
                        } else {
                            Some(Transition::Down {
                                reason,
                                consecutive: self.fails,
                            })
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    /// When the next reminder is due (only while DOWN with reminders enabled).
    pub fn next_reminder(&self) -> Option<Instant> {
        match (self.state, self.t.remind_every, self.last_alert) {
            (State::Down, Some(every), Some(last)) => Some(last + every),
            _ => None,
        }
    }

    pub fn on_tick(&mut self, now: Instant) -> Option<Transition> {
        let due = self.next_reminder()?;
        if now < due {
            return None;
        }
        self.last_alert = Some(now);
        Some(Transition::Remind {
            downtime: now.duration_since(self.down_since?),
            reason: self.last_reason.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(down: u32, up: u32, remind: Option<u64>) -> Health {
        Health::new(Thresholds {
            down_after: down,
            up_after: up,
            remind_every: remind.map(Duration::from_secs),
        })
    }
    fn fail(s: &str) -> Signal {
        Signal::Failure(s.into())
    }

    #[test]
    fn down_after_exactly_n_and_only_once() {
        let mut m = h(3, 1, None);
        let t0 = Instant::now();
        assert_eq!(
            m.on_signal(Signal::Success, t0),
            Some(Transition::Initial { up: true, reason: String::new() })
        );
        assert_eq!(m.on_signal(fail("a"), t0), None);
        assert_eq!(m.on_signal(fail("b"), t0), None);
        assert_eq!(
            m.on_signal(fail("c"), t0),
            Some(Transition::Down { reason: "c".into(), consecutive: 3 })
        );
        assert_eq!(m.state(), State::Down);
        assert_eq!(m.on_signal(fail("d"), t0), None);
        assert_eq!(m.last_reason(), "d");
    }

    #[test]
    fn blip_shorter_than_threshold_never_alerts() {
        let mut m = h(3, 1, None);
        let t0 = Instant::now();
        m.on_signal(Signal::Success, t0);
        assert_eq!(m.on_signal(fail("x"), t0), None);
        assert_eq!(m.on_signal(fail("x"), t0), None);
        assert_eq!(m.on_signal(Signal::Success, t0), None);
        assert_eq!(m.on_signal(fail("x"), t0), None);
        assert_eq!(m.on_signal(fail("x"), t0), None);
        assert_eq!(m.state(), State::Up);
    }

    #[test]
    fn up_after_m_successes_with_interleaved_failures() {
        let mut m = h(1, 3, None);
        let t0 = Instant::now();
        m.on_signal(Signal::Success, t0);
        assert!(matches!(m.on_signal(fail("x"), t0), Some(Transition::Down { .. })));
        assert_eq!(m.on_signal(Signal::Success, t0), None);
        assert_eq!(m.on_signal(Signal::Success, t0), None);
        assert_eq!(m.on_signal(fail("y"), t0), None, "resets the success count");
        assert_eq!(m.on_signal(Signal::Success, t0), None);
        assert_eq!(m.on_signal(Signal::Success, t0), None);
        let t1 = t0 + Duration::from_secs(90);
        assert_eq!(
            m.on_signal(Signal::Success, t1),
            Some(Transition::Up { downtime: Duration::from_secs(90), successes: 3 })
        );
        assert_eq!(m.state(), State::Up);
        assert!(m.down_since().is_none());
    }

    #[test]
    fn initial_down_path() {
        let mut m = h(2, 1, None);
        let t0 = Instant::now();
        assert_eq!(m.on_signal(fail("refused"), t0), None);
        assert_eq!(
            m.on_signal(fail("refused"), t0),
            Some(Transition::Initial { up: false, reason: "refused".into() })
        );
        assert_eq!(m.state(), State::Down);
        assert!(matches!(m.on_signal(Signal::Success, t0), Some(Transition::Up { .. })));
    }

    #[test]
    fn reminders_follow_cadence_and_stop_on_up() {
        let mut m = h(1, 1, Some(60));
        let t0 = Instant::now();
        m.on_signal(Signal::Success, t0);
        m.on_signal(fail("x"), t0);
        assert_eq!(m.next_reminder(), Some(t0 + Duration::from_secs(60)));
        assert_eq!(m.on_tick(t0 + Duration::from_secs(59)), None);
        assert_eq!(
            m.on_tick(t0 + Duration::from_secs(60)),
            Some(Transition::Remind { downtime: Duration::from_secs(60), reason: "x".into() })
        );
        assert_eq!(m.next_reminder(), Some(t0 + Duration::from_secs(120)));
        m.on_signal(Signal::Success, t0 + Duration::from_secs(70));
        assert_eq!(m.next_reminder(), None);
        assert_eq!(m.on_tick(t0 + Duration::from_secs(200)), None);
    }

    #[test]
    fn no_reminders_when_disabled_and_zero_thresholds_are_clamped() {
        let mut m = h(0, 0, Some(0));
        let t0 = Instant::now();
        assert!(matches!(m.on_signal(fail("x"), t0), Some(Transition::Initial { up: false, .. })));
        assert_eq!(m.next_reminder(), None);
        assert_eq!(m.on_tick(t0 + Duration::from_secs(3600)), None);
    }
}
