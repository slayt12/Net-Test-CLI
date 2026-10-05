//! systemd backend helpers: `systemctl` wrappers and unit-file I/O. The install/uninstall
//! sequence itself stays in each binary because the files it touches differ.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::ServiceError;

pub fn require_systemd() -> Result<(), ServiceError> {
    if Path::new("/run/systemd/system").is_dir() {
        Ok(())
    } else {
        Err(ServiceError::usage(
            "systemd not detected (/run/systemd/system missing); no other init system is supported",
        ))
    }
}

/// Run `systemctl <args>`, returning stdout; a non-zero exit is a manager error with stderr.
pub fn systemctl(args: &[&str]) -> Result<String, ServiceError> {
    let out = Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ServiceError::manager(format!("could not run systemctl: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(ServiceError::manager(format!(
            "systemctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// `active`, `inactive`, `failed`, ... or `unknown` when systemctl cannot be run.
pub fn active_state(name: &str) -> String {
    Command::new("systemctl")
        .args(["is-active", name])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// `systemctl status` output; it exits non-zero for an inactive unit, so the text is returned
/// regardless of the exit code.
pub fn status_text(name: &str) -> std::io::Result<String> {
    let st = Command::new("systemctl")
        .args(["status", "--no-pager", "--lines=5", name])
        .output()?;
    Ok(String::from_utf8_lossy(&st.stdout).trim_end().to_string())
}

pub fn write_unit(unit_path: &Path, text: &str) -> Result<(), ServiceError> {
    if let Some(dir) = unit_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(unit_path, text)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(unit_path, std::fs::Permissions::from_mode(0o644))?;
    Ok(())
}

/// Write (or rewrite) the unit, reload, and enable + start it.
pub fn install_unit(unit_path: &Path, text: &str, name: &str) -> Result<(), ServiceError> {
    write_unit(unit_path, text)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", name])?;
    Ok(())
}

/// Disable + stop (tolerating an already stopped unit), delete the file, reload.
/// `Ok(false)` when the unit file did not exist.
pub fn remove_unit(unit_path: &Path, name: &str) -> Result<bool, ServiceError> {
    if !unit_path.exists() {
        return Ok(false);
    }
    let _ = Command::new("systemctl")
        .args(["disable", "--now", name])
        .stdin(Stdio::null())
        .status();
    std::fs::remove_file(unit_path)?;
    systemctl(&["daemon-reload"])?;
    Ok(true)
}
