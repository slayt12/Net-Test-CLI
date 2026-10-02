use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

use super::*;

#[test]
fn header_round_trip_every_kind() {
    for kind in Kind::ALL {
        let h = Header::new(kind)
            .with_seq(0xDEAD_BEEF_0000_0001)
            .with_send_ns(123_456_789_012)
            .with_flags(0xA5A5);
        let mut raw = [0u8; HEADER_LEN];
        let mut h2 = h;
        h2.payload_len = 77;
        h2.encode(&mut raw);
        let back = Header::decode(&raw).unwrap();
        assert_eq!(back, h2);
        assert_eq!(Kind::try_from(kind as u8).unwrap(), kind);
    }
}

#[test]
fn rejects_bad_magic_version_kind_size() {
    let mut raw = [0u8; HEADER_LEN];
    Header::new(Kind::Probe).encode(&mut raw);
    let mut bad = raw;
    bad[0] = b'X';
    assert_eq!(Header::decode(&bad), Err(FrameError::BadMagic));
    let mut bad = raw;
    bad[4] = 9;
    assert_eq!(Header::decode(&bad), Err(FrameError::BadVersion(9)));
    let mut bad = raw;
    bad[5] = 200;
    assert_eq!(Header::decode(&bad), Err(FrameError::BadKind(200)));
    let mut bad = raw;
    bad[24..28].copy_from_slice(&(MAX_PAYLOAD + 1).to_le_bytes());
    assert!(matches!(Header::decode(&bad), Err(FrameError::TooLarge(_))));
    assert!(matches!(
        Header::decode(&raw[..10]),
        Err(FrameError::Truncated { .. })
    ));
}

#[test]
fn frame_parse_and_to_bytes() {
    let f = Frame::new(Header::new(Kind::Echo).with_seq(7), vec![1u8, 2, 3]);
    let b = f.to_bytes();
    assert_eq!(b.len(), HEADER_LEN + 3);
    let back = Frame::parse(&b).unwrap();
    assert_eq!(back, f);
    assert!(matches!(
        Frame::parse(&b[..HEADER_LEN + 1]),
        Err(FrameError::Truncated { .. })
    ));
}

#[test]
fn codec_handles_byte_at_a_time() {
    let f1 = Frame::new(Header::new(Kind::Probe).with_seq(1), vec![9u8; 40]);
    let f2 = Frame::empty(Kind::Bye);
    let mut wire = BytesMut::new();
    let mut codec = NtCodec;
    codec.encode(f1.clone(), &mut wire).unwrap();
    codec.encode(f2.clone(), &mut wire).unwrap();

    let mut rx = BytesMut::new();
    let mut got = Vec::new();
    for b in wire.iter() {
        rx.extend_from_slice(&[*b]);
        while let Some(f) = codec.decode(&mut rx).unwrap() {
            got.push(f);
        }
    }
    assert_eq!(got, vec![f1, f2]);
    assert!(rx.is_empty());
}
