//! Fallback for platforms without a supported service manager.

use std::path::PathBuf;
use std::process::ExitCode;

use super::{InstallArgs, Report, ServiceError, ServicePaths};

fn unsupported() -> Result<(), ServiceError> {
    Err(ServiceError::usage(
        "service mode is supported on Linux (systemd) and Windows only; run `nettest-client monitor` in the foreground instead",
    ))
}

pub fn install(_: InstallArgs, _: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn uninstall(_: bool, _: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn start(_: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn stop(_: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn restart(_: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn status(_: &mut Report) -> Result<(), ServiceError> {
    unsupported()
}
pub fn run_as_service(_: PathBuf) -> ExitCode {
    eprintln!("'service run' is the Windows service entry point");
    ExitCode::from(super::EXIT_USAGE)
}

pub fn probe(_paths: &ServicePaths) -> (bool, bool, String) {
    (false, false, "unsupported platform".into())
}
