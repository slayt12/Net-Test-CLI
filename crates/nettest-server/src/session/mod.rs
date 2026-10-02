//! Per-client session state and the protocol handler shared by every listener.

pub mod auth;
pub mod echo;
pub mod table;

pub use table::{SessionId, SessionStats, SessionTable};
