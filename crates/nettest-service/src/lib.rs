//! nettest-service: the platform-generic half of "run this program as a system service", shared
//! by `nettest-server service ...` and `nettest-client service ...`.
//!
//! What lives here is independent of *which* program is installed: privilege elevation, console
//! output that survives the Windows UAC hop (`Report`), binary copy/removal, systemd unit
//! rendering plus `systemctl`, and the Windows Service Control Manager helpers including the one
//! `define_windows_service!` entry point per process. Each binary keeps its own config handling,
//! paths and messages.
//!
//! Invariants:
//! - Exit codes: `EXIT_MANAGER` (2) for a service-manager failure, `EXIT_USAGE` (3) for anything
//!   the user has to fix (not root, not supported, bad config). Both binaries document them.
//! - `run_elevated` never prompts itself; it re-executes through sudo / UAC and otherwise fails.
//! - Nothing here reads a config file: the caller validates and hands over paths.

pub mod elevate;
pub mod error;
pub mod files;
pub mod report;
pub mod systemd;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::Path;
use std::process::ExitCode;

pub use error::ServiceError;
pub use report::Report;

pub const EXIT_MANAGER: u8 = 2;
pub const EXIT_USAGE: u8 = 3;

/// Identity of one installable service. Each binary defines a `static` of these.
#[derive(Debug, Clone, Copy)]
pub struct ServiceSpec {
    /// systemd unit name / SCM service name, e.g. `nettest-server`.
    pub name: &'static str,
    /// SCM display name.
    pub display_name: &'static str,
    /// Unit `Description=` / SCM description.
    pub description: &'static str,
    /// Executable name as the user types it, for messages.
    pub program: &'static str,
}

/// Run `body` with a `Report`, elevating first when `needs_root` and this process is not already
/// root / administrator. The elevated child re-runs the same command line (plus the hidden
/// `--capture-output` on Windows) and its exit code is returned verbatim.
pub fn run_elevated(
    needs_root: bool,
    capture_output: Option<&Path>,
    body: impl FnOnce(&mut Report) -> Result<(), ServiceError>,
) -> ExitCode {
    let mut out = Report::new(capture_output);
    if needs_root && !elevate::is_elevated() {
        let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        return match elevate::reexec_elevated(&argv) {
            Ok(code) => code,
            Err(e) => {
                out.err(format!("could not obtain administrator rights: {e}"));
                ExitCode::from(EXIT_USAGE)
            }
        };
    }
    match body(&mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            out.err(format!("error: {e}"));
            ExitCode::from(e.code)
        }
    }
}
