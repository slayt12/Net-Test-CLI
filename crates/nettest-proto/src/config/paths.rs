//! Per-user locations. Falls back to the current directory when the platform has no config dir
//! (rare, but a diagnostics tool must never refuse to start over it).

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nettest")
}

pub fn client_config_path() -> PathBuf {
    config_dir().join("client.toml")
}

pub fn server_config_path() -> PathBuf {
    config_dir().join("server.toml")
}

pub fn server_cert_dir() -> PathBuf {
    config_dir().join("server")
}
