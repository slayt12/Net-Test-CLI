//! TLS helpers built on rustls with the `ring` provider.
//!
//! Why not system TLS: the binaries must run on a fresh Windows or Linux box with nothing
//! installed, and a self-signed certificate generated on first run is good enough for a
//! diagnostics tool. Trust is established by fingerprint pinning (or explicitly disabled).

pub mod client;
pub mod fingerprint;
pub mod server;

use std::sync::Arc;

/// The one and only crypto provider in the binary. Using `builder_with_provider` everywhere
/// avoids rustls' "multiple providers" panic if a future dependency enables aws-lc-rs.
pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}
