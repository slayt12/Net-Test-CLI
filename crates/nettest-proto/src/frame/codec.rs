//! tokio-util codec for the raw TCP transport.
//!
//! TCP has no message boundaries, so the header's `payload_len` is the delimiter. The decoder
//! peeks the header without consuming it until the whole payload has arrived, which keeps
//! partial reads trivially correct.

use bytes::{Buf, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::{Frame, FrameError, HEADER_LEN, Header, encode_into};

#[derive(Debug, Default, Clone, Copy)]
pub struct NtCodec;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Decoder for NtCodec {
    type Item = Frame;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }
        let header = Header::decode(&src[..HEADER_LEN])?;
        let total = HEADER_LEN + header.payload_len as usize;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }
        src.advance(HEADER_LEN);
        let payload = src.split_to(header.payload_len as usize).freeze();
        Ok(Some(Frame { header, payload }))
    }
}

impl Encoder<Frame> for NtCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        if item.payload.len() as u64 > super::MAX_PAYLOAD as u64 {
            return Err(FrameError::TooLarge(item.payload.len() as u32).into());
        }
        encode_into(&item.header, &item.payload, dst);
        Ok(())
    }
}
