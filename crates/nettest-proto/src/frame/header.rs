//! 32-byte little-endian frame header.
//!
//! ```text
//! off size field
//! 0   4    magic "NTP1"
//! 4   1    version
//! 5   1    kind
//! 6   2    flags
//! 8   8    seq
//! 16  8    client_send_ns   client monotonic ns, echoed verbatim by the server
//! 24  4    payload_len
//! 28  4    reserved (0)
//! ```
//! Encode/decode never allocate so the hot probe path stays cheap on all transports.

use super::Kind;

pub const MAGIC: [u8; 4] = *b"NTP1";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 32;
/// Hard cap for TCP/WS payloads. UDP is further limited by the datagram size the caller picks.
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Flag bits (per-kind meaning).
pub mod flags {
    /// `TpStart`: 0 = client uploads to server, 1 = server streams to client.
    pub const TP_DOWNLOAD: u16 = 0b0000_0001;
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FrameError {
    #[error("bad magic (not a nettest frame)")]
    BadMagic,
    #[error("unsupported protocol version {0}")]
    BadVersion(u8),
    #[error("unknown frame kind {0}")]
    BadKind(u8),
    #[error("payload length {0} exceeds limit")]
    TooLarge(u32),
    #[error("truncated frame: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub flags: u16,
    pub seq: u64,
    pub client_send_ns: u64,
    pub payload_len: u32,
}

impl Header {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            flags: 0,
            seq: 0,
            client_send_ns: 0,
            payload_len: 0,
        }
    }

    pub fn with_seq(mut self, seq: u64) -> Self {
        self.seq = seq;
        self
    }

    pub fn with_send_ns(mut self, ns: u64) -> Self {
        self.client_send_ns = ns;
        self
    }

    pub fn with_flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    pub fn encode(&self, out: &mut [u8; HEADER_LEN]) {
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = self.kind as u8;
        out[6..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..16].copy_from_slice(&self.seq.to_le_bytes());
        out[16..24].copy_from_slice(&self.client_send_ns.to_le_bytes());
        out[24..28].copy_from_slice(&self.payload_len.to_le_bytes());
        out[28..32].copy_from_slice(&0u32.to_le_bytes());
    }

    /// Decode from the first `HEADER_LEN` bytes of `buf`.
    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        if buf.len() < HEADER_LEN {
            return Err(FrameError::Truncated {
                need: HEADER_LEN,
                have: buf.len(),
            });
        }
        if buf[0..4] != MAGIC {
            return Err(FrameError::BadMagic);
        }
        if buf[4] != VERSION {
            return Err(FrameError::BadVersion(buf[4]));
        }
        let kind = Kind::try_from(buf[5])?;
        let flags = u16::from_le_bytes([buf[6], buf[7]]);
        let seq = u64::from_le_bytes(buf[8..16].try_into().expect("8 bytes"));
        let client_send_ns = u64::from_le_bytes(buf[16..24].try_into().expect("8 bytes"));
        let payload_len = u32::from_le_bytes(buf[24..28].try_into().expect("4 bytes"));
        if payload_len > MAX_PAYLOAD {
            return Err(FrameError::TooLarge(payload_len));
        }
        Ok(Self {
            kind,
            flags,
            seq,
            client_send_ns,
            payload_len,
        })
    }
}
