//! Monotonic nanosecond clock shared by client and server.
//!
//! `client_send_ns` on the wire is a client-local monotonic value. It only has to be
//! comparable with itself on the same machine, so a process-start-relative `Instant` is enough
//! and avoids wall-clock jumps corrupting RTT measurements.

use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Process-relative monotonic epoch; first call fixes it.
pub fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

/// Nanoseconds since [`epoch`]. Saturates at u64::MAX after ~584 years.
pub fn now_ns() -> u64 {
    epoch().elapsed().as_nanos().min(u64::MAX as u128) as u64
}
