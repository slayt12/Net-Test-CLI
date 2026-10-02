//! Wire format: a fixed 32-byte little-endian header followed by `payload_len` bytes.
//!
//! The same frame is used on TCP (length-delimited by the header), WebSocket (one binary message
//! per frame) and UDP (one datagram per frame), so transports differ only in delivery, never in
//! parsing.

mod codec;
mod header;
mod kind;
pub mod payloads;

pub use codec::NtCodec;
pub mod codec_error {
    pub use super::codec::CodecError;
}
pub use header::{FrameError, HEADER_LEN, Header, MAGIC, MAX_PAYLOAD, VERSION, flags};
pub use kind::Kind;

use bytes::{Bytes, BytesMut};

/// A decoded frame. `payload` is zero-copy from the receive buffer where the transport allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: Header,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(header: Header, payload: impl Into<Bytes>) -> Self {
        let payload = payload.into();
        let mut header = header;
        header.payload_len = payload.len() as u32;
        Self { header, payload }
    }

    /// Header-only frame.
    pub fn empty(kind: Kind) -> Self {
        Self::new(Header::new(kind), Bytes::new())
    }

    /// Serialize header + payload into a fresh buffer.
    pub fn to_bytes(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(HEADER_LEN + self.payload.len());
        encode_into(&self.header, &self.payload, &mut out);
        out.freeze()
    }

    /// Parse a complete frame from a single buffer (UDP datagram / WS message).
    pub fn parse(buf: &[u8]) -> Result<Self, FrameError> {
        let header = Header::decode(buf)?;
        let end = HEADER_LEN + header.payload_len as usize;
        if buf.len() < end {
            return Err(FrameError::Truncated {
                need: end,
                have: buf.len(),
            });
        }
        Ok(Self {
            header,
            payload: Bytes::copy_from_slice(&buf[HEADER_LEN..end]),
        })
    }
}

/// Append header + payload to `out`. `header.payload_len` is overwritten with `payload.len()`.
pub fn encode_into(header: &Header, payload: &[u8], out: &mut BytesMut) {
    let mut h = *header;
    h.payload_len = payload.len() as u32;
    let mut raw = [0u8; HEADER_LEN];
    h.encode(&mut raw);
    out.extend_from_slice(&raw);
    out.extend_from_slice(payload);
}

#[cfg(test)]
mod tests;
