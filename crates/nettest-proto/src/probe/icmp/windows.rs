//! Windows ICMP via `IcmpSendEcho2` / `Icmp6SendEcho2` (iphlpapi.dll, a system DLL that needs
//! no administrator rights, unlike raw sockets).
//!
//! The call is synchronous, so each probe runs on the blocking pool; RTT is taken with our own
//! clock because the API reports whole milliseconds. Only the `Status` field of the reply is
//! read, at its fixed offset, so the 32/64-bit layout difference of `ICMP_ECHO_REPLY` does not
//! matter.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    ICMP_ECHO_REPLY, ICMPV6_ECHO_REPLY_LH, IP_OPTION_INFORMATION, Icmp6CreateFile, Icmp6SendEcho2,
    IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET6, IN6_ADDR, IN6_ADDR_0, SOCKADDR_IN6, SOCKADDR_IN6_0,
};

use super::{data_len_for, packet};
use crate::probe::{ProbeError, ProbeReply, ProbeTarget};

/// `HANDLE` is a raw pointer; the ICMP handle is documented as usable from any thread.
struct IcmpHandle(HANDLE);
unsafe impl Send for IcmpHandle {}
unsafe impl Sync for IcmpHandle {}

impl Drop for IcmpHandle {
    fn drop(&mut self) {
        unsafe {
            IcmpCloseHandle(self.0);
        }
    }
}

pub struct IcmpProber {
    handle: Arc<IcmpHandle>,
    target: IpAddr,
    source6: Ipv6Addr,
    data_len: usize,
    timeout: Duration,
}

impl IcmpProber {
    pub async fn open(target: IpAddr, t: &ProbeTarget) -> Result<Self, ProbeError> {
        let h = unsafe {
            if target.is_ipv6() {
                Icmp6CreateFile()
            } else {
                IcmpCreateFile()
            }
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(ProbeError::Io(std::io::Error::last_os_error()));
        }
        let source6 = match t.bind {
            Some(IpAddr::V6(ip)) => ip,
            _ => Ipv6Addr::UNSPECIFIED,
        };
        Ok(Self {
            handle: Arc::new(IcmpHandle(h)),
            target,
            source6,
            data_len: data_len_for(t.payload_bytes),
            timeout: t.timeout,
        })
    }

    pub async fn probe(&self, seq: u64) -> Result<ProbeReply, ProbeError> {
        let handle = self.handle.clone();
        let target = self.target;
        let source6 = self.source6;
        let data = packet::echo_data(seq, crate::time::now_ns(), self.data_len);
        let timeout_ms = self.timeout.as_millis().clamp(1, u32::MAX as u128) as u32;
        let t0 = Instant::now();
        let res = tokio::task::spawn_blocking(move || {
            send_echo(&handle, target, source6, &data, timeout_ms)
        })
        .await
        .map_err(|e| ProbeError::Io(std::io::Error::other(e)))?;
        res.map(|()| ProbeReply {
            rtt: t0.elapsed(),
            detail: None,
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "icmp echo to {} ({} data bytes, IcmpSendEcho2)",
            self.target, self.data_len
        )
    }
}

fn sockaddr6(ip: Ipv6Addr) -> SOCKADDR_IN6 {
    SOCKADDR_IN6 {
        sin6_family: AF_INET6,
        sin6_port: 0,
        sin6_flowinfo: 0,
        sin6_addr: IN6_ADDR {
            u: IN6_ADDR_0 { Byte: ip.octets() },
        },
        Anonymous: SOCKADDR_IN6_0 { sin6_scope_id: 0 },
    }
}

fn send_echo(
    h: &IcmpHandle,
    target: IpAddr,
    source6: Ipv6Addr,
    data: &[u8],
    timeout_ms: u32,
) -> Result<(), ProbeError> {
    let opts = IP_OPTION_INFORMATION {
        Ttl: 128,
        Tos: 0,
        Flags: 0,
        OptionsSize: 0,
        OptionsData: std::ptr::null_mut(),
    };
    // Documented minimum: one reply struct plus the echoed data plus 8 bytes for an ICMP error.
    let (replies, status_offset) = match target {
        IpAddr::V4(ip) => {
            let mut buf =
                vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + data.len().max(16) + 8];
            let n = unsafe {
                IcmpSendEcho2(
                    h.0,
                    std::ptr::null_mut(),
                    None,
                    std::ptr::null(),
                    u32::from_ne_bytes(ip.octets()),
                    data.as_ptr().cast(),
                    data.len() as u16,
                    &opts,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    timeout_ms,
                )
            };
            if n == 0 {
                return map_status(unsafe { GetLastError() });
            }
            // ICMP_ECHO_REPLY: Address u32, Status u32, ...
            (buf, 4)
        }
        IpAddr::V6(ip) => {
            let mut buf = vec![0u8; std::mem::size_of::<ICMPV6_ECHO_REPLY_LH>() + data.len() + 8];
            let src = sockaddr6(source6);
            let dst = sockaddr6(ip);
            let n = unsafe {
                Icmp6SendEcho2(
                    h.0,
                    std::ptr::null_mut(),
                    None,
                    std::ptr::null(),
                    &src,
                    &dst,
                    data.as_ptr().cast(),
                    data.len() as u16,
                    &opts,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    timeout_ms,
                )
            };
            if n == 0 {
                return map_status(unsafe { GetLastError() });
            }
            // ICMPV6_ECHO_REPLY_LH: packed IPV6_ADDRESS_EX (26 bytes) then Status u32.
            (buf, std::mem::offset_of!(ICMPV6_ECHO_REPLY_LH, Status))
        }
    };
    let status = u32::from_ne_bytes(
        replies[status_offset..status_offset + 4]
            .try_into()
            .expect("4 bytes"),
    );
    map_status(status)
}

/// IP_STATUS codes from ipexport.h.
fn map_status(code: u32) -> Result<(), ProbeError> {
    match code {
        0 => Ok(()),
        11010 => Err(ProbeError::Timeout),
        11002 => Err(ProbeError::Unreachable("network unreachable".into())),
        11003 => Err(ProbeError::Unreachable("host unreachable".into())),
        11004 => Err(ProbeError::Unreachable("protocol unreachable".into())),
        11005 => Err(ProbeError::Unreachable("port unreachable".into())),
        11013 => Err(ProbeError::Unreachable("ttl expired in transit".into())),
        11018 => Err(ProbeError::Unreachable("bad destination".into())),
        11040 => Err(ProbeError::Unreachable("destination unreachable".into())),
        other => Err(ProbeError::Io(std::io::Error::from_raw_os_error(
            other as i32,
        ))),
    }
}
