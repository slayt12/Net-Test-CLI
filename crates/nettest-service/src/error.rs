//! Failure with the exit code it maps to.

use crate::{EXIT_MANAGER, EXIT_USAGE};

#[derive(Debug)]
pub struct ServiceError {
    pub code: u8,
    pub msg: String,
}

impl ServiceError {
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            msg: msg.into(),
        }
    }
    pub fn manager(msg: impl Into<String>) -> Self {
        Self {
            code: EXIT_MANAGER,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        ServiceError::manager(e.to_string())
    }
}

impl From<nettest_proto::config::ConfigError> for ServiceError {
    fn from(e: nettest_proto::config::ConfigError) -> Self {
        ServiceError::usage(e.to_string())
    }
}

#[cfg(windows)]
impl From<windows_service::Error> for ServiceError {
    fn from(e: windows_service::Error) -> Self {
        ServiceError::manager(e.to_string())
    }
}
