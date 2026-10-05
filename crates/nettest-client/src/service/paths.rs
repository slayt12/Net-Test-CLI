//! System-wide locations for the monitor service. Shares `/etc/nettest` and
//! `%ProgramData%\nettest` with nettest-server but never a file name, and on Linux uses its own
//! `LogsDirectory` (a `DynamicUser=` unit re-chowns that directory at every start, so two units
//! cannot share one).

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServicePaths {
    /// Where the executable is copied on install.
    pub bin: PathBuf,
    /// The installed `monitor.toml` (may hold tokens: private permissions).
    pub config: PathBuf,
    pub log_file: PathBuf,
    /// systemd unit file (unused on Windows).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub unit: PathBuf,
    /// Whether the log file's directory belongs to the monitor alone (purge removes it).
    pub log_dir_is_own: bool,
}

#[cfg(target_os = "linux")]
pub fn service_paths() -> ServicePaths {
    ServicePaths {
        bin: PathBuf::from("/usr/local/bin/nettest-client"),
        config: PathBuf::from("/etc/nettest/monitor.toml"),
        // LogsDirectory=nettest-monitor in the unit creates and owns this for the dynamic user.
        log_file: PathBuf::from("/var/log/nettest-monitor/monitor.log"),
        unit: PathBuf::from("/etc/systemd/system/nettest-monitor.service"),
        log_dir_is_own: true,
    }
}

#[cfg(windows)]
pub fn service_paths() -> ServicePaths {
    let program_data =
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()));
    let program_files = PathBuf::from(
        std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into()),
    );
    let data = program_data.join("nettest");
    ServicePaths {
        bin: program_files.join("nettest").join("nettest-client.exe"),
        config: data.join("monitor.toml"),
        log_file: data.join("monitor.log"),
        unit: PathBuf::new(),
        log_dir_is_own: false,
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn service_paths() -> ServicePaths {
    ServicePaths {
        bin: PathBuf::from("/usr/local/bin/nettest-client"),
        config: PathBuf::from("/etc/nettest/monitor.toml"),
        log_file: PathBuf::from("/var/log/nettest-monitor/monitor.log"),
        unit: PathBuf::new(),
        log_dir_is_own: true,
    }
}
