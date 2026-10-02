//! Exponential backoff for reconnects: 500 ms doubling to a 30 s cap.

use std::time::Duration;

pub struct Backoff {
    current: Duration,
    pub attempt: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            current: Duration::from_millis(500),
            attempt: 0,
        }
    }

    pub fn next(&mut self) -> Duration {
        self.attempt += 1;
        let d = self.current;
        self.current = (self.current * 2).min(Duration::from_secs(30));
        d
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}
