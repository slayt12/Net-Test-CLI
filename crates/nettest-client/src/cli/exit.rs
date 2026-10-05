//! Exit codes for headless runs. Documented in README; scripts depend on these staying stable.
//!
//! `nettest-client monitor` reuses them: 0 clean stop (signal), 2 `--test-notify` could not
//! deliver to every notifier, 3 unreadable or invalid monitor.toml, 4 log file not writable.

use std::process::ExitCode;

pub const OK: u8 = 0;
/// Run completed but loss or latency exceeded the configured thresholds.
pub const THRESHOLD: u8 = 1;
/// Could not connect, authentication rejected, or reconnects exhausted.
pub const CONNECT: u8 = 2;
/// Bad arguments, unreadable config, TLS misconfiguration.
pub const USAGE: u8 = 3;
/// Could not write a log file or report.
pub const IO: u8 = 4;
/// Interrupted before a bounded run (count/duration) completed.
pub const INTERRUPTED: u8 = 5;

pub fn code(c: u8) -> ExitCode {
    ExitCode::from(c)
}
