//! ICMP echo request/reply layout and checksum, shared by the Linux socket path and the Windows
//! API path (which only sees the data portion).
//!
//! Data layout: `[seq u64 LE][send_ns u64 LE][filler]`. The 64-bit sequence lives in the data
//! because the header's 16-bit field wraps after 65535 probes and Linux ping sockets rewrite
//! the identifier, so the data is the only reliable match key.

pub const ECHO_HEADER: usize = 8;
pub const MIN_DATA: usize = 16;
/// IPv4 maximum (65535 - 20 IP - 8 ICMP).
pub const MAX_DATA: usize = 65507 - ECHO_HEADER;

const V4_ECHO_REQUEST: u8 = 8;
const V4_ECHO_REPLY: u8 = 0;
const V6_ECHO_REQUEST: u8 = 128;
const V6_ECHO_REPLY: u8 = 129;

/// RFC 1071 ones-complement sum over the whole ICMP message.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn echo_data(seq: u64, send_ns: u64, len: usize) -> Vec<u8> {
    let len = len.max(MIN_DATA);
    let mut d = Vec::with_capacity(len);
    d.extend_from_slice(&seq.to_le_bytes());
    d.extend_from_slice(&send_ns.to_le_bytes());
    let mut x = seq ^ 0x5DEE_CE66_D1CE_4F6D;
    while d.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        d.extend_from_slice(&x.to_le_bytes());
    }
    d.truncate(len);
    d
}

/// Full ICMP echo request. The IPv4 checksum is filled in; IPv6 leaves it zero because the
/// kernel computes it over the pseudo-header.
pub fn echo_request(v6: bool, ident: u16, seq: u64, send_ns: u64, data_len: usize) -> Vec<u8> {
    let data = echo_data(seq, send_ns, data_len);
    let mut pkt = Vec::with_capacity(ECHO_HEADER + data.len());
    pkt.push(if v6 { V6_ECHO_REQUEST } else { V4_ECHO_REQUEST });
    pkt.push(0);
    pkt.extend_from_slice(&[0, 0]);
    pkt.extend_from_slice(&ident.to_be_bytes());
    pkt.extend_from_slice(&(seq as u16).to_be_bytes());
    pkt.extend_from_slice(&data);
    if !v6 {
        let c = checksum(&pkt).to_be_bytes();
        pkt[2] = c[0];
        pkt[3] = c[1];
    }
    pkt
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EchoReply {
    pub ident: u16,
    pub seq: u64,
}

/// Parse a received message. `has_ip_header` is true for IPv4 raw sockets, which deliver the IP
/// header in front of the ICMP message; ping sockets and IPv6 raw sockets do not.
pub fn parse_reply(buf: &[u8], v6: bool, has_ip_header: bool) -> Option<EchoReply> {
    let buf = if has_ip_header {
        let ihl = usize::from(buf.first()? & 0x0f) * 4;
        buf.get(ihl..)?
    } else {
        buf
    };
    if buf.len() < ECHO_HEADER + MIN_DATA {
        return None;
    }
    let expected = if v6 { V6_ECHO_REPLY } else { V4_ECHO_REPLY };
    if buf[0] != expected {
        return None;
    }
    let ident = u16::from_be_bytes([buf[4], buf[5]]);
    let seq16 = u16::from_be_bytes([buf[6], buf[7]]);
    let seq = seq_from_data(&buf[ECHO_HEADER..])?;
    (seq as u16 == seq16).then_some(EchoReply { ident, seq })
}

/// The 64-bit sequence embedded in echo data (the Windows API hands back only the data).
pub fn seq_from_data(data: &[u8]) -> Option<u64> {
    let bytes: [u8; 8] = data.get(..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc1071_vector() {
        assert_eq!(
            checksum(&[0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7]),
            0x220d
        );
        let pkt = echo_request(false, 0x1234, 70000, 99, 56);
        assert_eq!(pkt.len(), 64);
        // A message with a valid checksum sums to zero.
        assert_eq!(checksum(&pkt), 0);
    }

    #[test]
    fn round_trip_v4_and_v6() {
        for v6 in [false, true] {
            let mut pkt = echo_request(v6, 7, 70001, 5, 32);
            pkt[0] = if v6 { 129 } else { 0 };
            let r = parse_reply(&pkt, v6, false).unwrap();
            assert_eq!(
                r,
                EchoReply {
                    ident: 7,
                    seq: 70001
                }
            );
            assert!(parse_reply(&pkt, !v6, false).is_none());
        }
    }

    #[test]
    fn raw_v4_skips_ip_header() {
        let mut pkt = echo_request(false, 1, 3, 0, 16);
        pkt[0] = 0;
        let mut framed = vec![0x45u8; 1];
        framed.extend_from_slice(&[0u8; 19]);
        framed.extend_from_slice(&pkt);
        assert_eq!(parse_reply(&framed, false, true).unwrap().seq, 3);
        assert!(parse_reply(&framed[..10], false, true).is_none());
    }
}
