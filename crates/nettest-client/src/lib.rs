//! nettest-client library: everything except the `main` that parses arguments.
//!
//! Exposed as a library (like the server) so integration tests can drive the monitor supervisor
//! against an in-process nettest-server. The binary in `main.rs` is a thin dispatcher.

pub mod cli;
pub mod monitor;
pub mod output;
pub mod runner;
pub mod service;
pub mod tui;
