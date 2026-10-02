//! JSON payload bodies for the control frames. Only sent once per connection (or once per
//! throughput run), so JSON's cost is irrelevant and its self-description helps debugging with
//! a packet capture.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HelloPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub client_id: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HelloAckPayload {
    pub session_id: u64,
    pub server_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TpStartPayload {
    /// Seconds the sender streams for.
    pub secs: u64,
    /// Bytes per `TpData` frame.
    pub chunk: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TpResultPayload {
    pub bytes: u64,
    pub elapsed_ns: u64,
    pub frames: u64,
}

pub fn to_json<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("payload structs always serialize")
}

pub fn from_json<'a, T: Deserialize<'a>>(b: &'a [u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(b)
}
