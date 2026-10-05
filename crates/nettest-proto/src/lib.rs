//! nettest-proto: everything shared between `nettest-client` and `nettest-server`.
//!
//! Purpose: a single wire format, transport layer, statistics engine, log sinks and report
//! generator so the two binaries can never drift apart. Nothing in here touches a terminal.
//!
//! Invariants:
//! - The wire header is exactly `frame::HEADER_LEN` bytes, little-endian, on every transport.
//! - Latency is always round-trip; the server echoes `client_send_ns` untouched and never
//!   injects its own clock, because the two machines are not assumed to be time-synced.
//! - TLS uses the `ring` provider only (see workspace Cargo.toml for why).

pub mod config;
pub mod frame;
pub mod http;
pub mod log;
pub mod probe;
pub mod report;
pub mod sinks;
pub mod stats;
pub mod time;
pub mod tls;
pub mod transport;

pub use bytes;
