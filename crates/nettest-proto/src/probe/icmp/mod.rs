//! `ping://host`: ICMP echo without privileges.
//!
//! Linux: a `SOCK_DGRAM` ICMP "ping socket" (allowed for groups in
//! `net.ipv4.ping_group_range`, which systemd opens for everyone on most distributions), falling
//! back to a raw socket when running as root. Windows: `IcmpSendEcho2` from iphlpapi, a system
//! DLL that needs no admin rights. Both paths share the packet builder in `packet`.

pub mod packet;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::IcmpProber;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::IcmpProber;

pub const PERMISSION_HINT: &str = "ICMP is not permitted for this user: add your group to \
net.ipv4.ping_group_range or run as root, or probe a port with connect://host:port instead";

/// ICMP data bytes for a requested message size (header included), clamped to what the
/// packet format and the IPv4 maximum allow.
pub fn data_len_for(payload_bytes: usize) -> usize {
    payload_bytes
        .saturating_sub(packet::ECHO_HEADER)
        .clamp(packet::MIN_DATA, packet::MAX_DATA)
}
