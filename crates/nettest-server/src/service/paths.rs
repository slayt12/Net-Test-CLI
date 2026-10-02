//! System-wide locations for the service. Deliberately separate from the per-user paths in
//! nettest-proto: a service must not depend on whichever user happened to install it.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServicePaths {
    /// Where the executable is copied on install.
    pub bin: PathBuf,
    /// The service's `server.toml` (holds the token: private permissions).
    pub config: PathBuf,
    /// Where `cert.pem` / `key.pem` are generated.
    pub cert_dir: PathBuf,
    pub log_file: PathBuf,
    /// systemd unit file (unused on Windows).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub unit: PathBuf,
}

#[cfg(target_os = "linux")]
pub fn service_paths() -> ServicePaths {
    ServicePaths {
        bin: PathBuf::from("/usr/local/bin/nettest-server"),
        config: PathBuf::from("/etc/nettest/server.toml"),
        // StateDirectory= / LogsDirectory= in the unit create and own these for the dynamic user.
        cert_dir: PathBuf::from("/var/lib/nettest"),
        log_file: PathBuf::from("/var/log/nettest/server.log"),
        unit: PathBuf::from("/etc/systemd/system/nettest-server.service"),
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
        bin: program_files.join("nettest").join("nettest-server.exe"),
        config: data.join("server.toml"),
        cert_dir: data.join("certs"),
        log_file: data.join("server.log"),
        unit: PathBuf::new(),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn service_paths() -> ServicePaths {
    ServicePaths {
        bin: PathBuf::from("/usr/local/bin/nettest-server"),
        config: PathBuf::from("/etc/nettest/server.toml"),
        cert_dir: PathBuf::from("/var/lib/nettest"),
        log_file: PathBuf::from("/var/log/nettest/server.log"),
        unit: PathBuf::new(),
    }
}
