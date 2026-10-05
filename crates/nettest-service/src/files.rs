//! File-system helpers for install / uninstall: copying the running executable into place and
//! removing exactly the files one service owns (two nettest services share `/etc/nettest` and
//! `%ProgramData%\nettest`, so a purge must never wipe the directory wholesale).

use std::path::{Path, PathBuf};

use crate::ServiceError;

/// Copy the running executable to the system location. The old file is unlinked first so a
/// binary that is currently executing (an upgrade) can be replaced.
pub fn copy_binary(dest: &Path) -> Result<PathBuf, ServiceError> {
    let src = std::env::current_exe()?;
    if same_file(&src, dest) {
        return Ok(dest.to_path_buf());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(dest);
    std::fs::copy(&src, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dest.to_path_buf())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Remove an installed executable, retrying because the Windows SCM may still hold the image
/// for a moment after the service reports Stopped. `Ok(false)` when it did not exist.
pub fn remove_binary(path: &Path, retries: u32) -> Result<bool, ServiceError> {
    if !path.exists() {
        return Ok(false);
    }
    let mut last = None;
    for _ in 0..retries.max(1) {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(true),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    Err(ServiceError::manager(format!(
        "could not remove {}: {}",
        path.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// `Ok(true)` when something was removed.
pub fn remove_file_if_exists(p: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Remove a directory tree. On Linux a `DynamicUser=` service's `StateDirectory`/`LogsDirectory`
/// is a symlink into `/var/{lib,log}/private/`; `remove_dir_all` would only unlink the symlink,
/// so the private target is removed as well.
pub fn remove_dir_all_if_exists(p: &Path) -> std::io::Result<bool> {
    let meta = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        if let Ok(target) = std::fs::read_link(p) {
            let target = if target.is_absolute() {
                target
            } else {
                p.parent().map(|d| d.join(&target)).unwrap_or(target)
            };
            let _ = std::fs::remove_dir_all(&target);
        }
        std::fs::remove_file(p)?;
    } else {
        std::fs::remove_dir_all(p)?;
    }
    Ok(true)
}

/// Remove `p` only when it is an empty directory. `Ok(true)` when removed.
pub fn remove_dir_if_empty(p: &Path) -> std::io::Result<bool> {
    match std::fs::read_dir(p) {
        Ok(mut it) => {
            if it.next().is_none() {
                std::fs::remove_dir(p)?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Write `bytes` to `path` readable by the owner only (0600 on unix; on Windows the file
/// inherits the ACL of `%ProgramData%\nettest`, which is administrators-only by default).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_removal() {
        let dir = std::env::temp_dir().join(format!("nettest-files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!remove_dir_if_empty(&dir).unwrap());
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert!(!remove_dir_if_empty(&dir).unwrap());
        assert!(remove_dir_if_empty(&dir.join("sub")).unwrap());
        assert!(remove_dir_if_empty(&dir).unwrap());
        assert!(!dir.exists());
        assert!(!remove_file_if_exists(&dir.join("nope")).unwrap());
        assert!(!remove_dir_all_if_exists(&dir).unwrap());
    }
}
