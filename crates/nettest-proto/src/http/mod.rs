//! Minimal HTTP/1.1 client: one `POST` per call, used by the monitor to deliver webhooks
//! (ntfy, Slack, Discord). Hand-rolled over the same TCP + rustls stack as the transports so the
//! binary gains no HTTP framework and keeps a single crypto provider.
//!
//! Invariants:
//! - Every request carries `Connection: close`; the response body ends at `Content-Length` or
//!   EOF, so chunked decoding is never needed for the APIs we talk to.
//! - Header values from callers are validated (ASCII, no CR/LF) so a notification text can never
//!   smuggle headers.
//! - The whole exchange runs under one timeout.

mod client;
mod url;

pub use client::{HttpError, HttpOptions, Response, post};
pub use url::{Url, UrlError};
