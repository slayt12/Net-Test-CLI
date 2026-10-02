//! Persistent settings for both binaries (TOML in the per-user config directory).
//!
//! Precedence is decided by the binaries: CLI flag > environment > file > default.

mod client;
mod paths;
mod server;

pub use client::{ClientConfig, SinkConfig, TestMode};
pub use paths::{client_config_path, config_dir, server_cert_dir, server_config_path};
pub use server::{DEFAULT_BANNER, ServerConfig};

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
}

pub fn load<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, ConfigError> {
    if !path.exists() {
        return Ok(T::default());
    }
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.display().to_string(),
        source,
    })
}

pub fn save<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let text = toml::to_string_pretty(value)?;
    std::fs::write(path, text).map_err(|source| ConfigError::Write {
        path: path.display().to_string(),
        source,
    })
}

/// Like `save`, but the file is created readable by its owner only (0600) on Unix. Used for
/// system-wide server configs because they hold the shared token. On Windows the file inherits
/// the directory ACL; callers choose a protected directory.
pub fn save_private<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), ConfigError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let text = toml::to_string_pretty(value)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = opts.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(text.as_bytes())
    };
    write().map_err(|source| ConfigError::Write {
        path: path.display().to_string(),
        source,
    })
}
