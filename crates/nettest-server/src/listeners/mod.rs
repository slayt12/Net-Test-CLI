//! Bind sockets and spawn one accept/recv loop per protocol. Every listener first classifies
//! what connected (see `banner`) so scanners get an identification and clients get the echo.

pub mod banner;
pub mod tcp;
pub mod udp;
pub mod ws;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use nettest_proto::transport::Protocol;

use crate::ServerCtx;

/// Expand the `bind` setting into concrete addresses. `any` binds v4 and v6 separately because
/// dual-stack behaviour of `[::]` differs between Linux and Windows.
pub fn bind_addrs(bind: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
    let b = bind.trim();
    if b.is_empty() || b.eq_ignore_ascii_case("any") || b == "*" {
        return Ok(vec![
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port),
            SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port),
        ]);
    }
    let ip: IpAddr = b
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid bind address '{b}'"))?;
    Ok(vec![SocketAddr::new(ip, port)])
}

pub async fn spawn_all(ctx: Arc<ServerCtx>) -> anyhow::Result<Vec<(Protocol, SocketAddr)>> {
    let mut bound = Vec::new();
    let cfg = &ctx.config;

    if cfg.tcp_port != 0 {
        for addr in bind_addrs(&cfg.bind, cfg.tcp_port)? {
            if let Some(a) = tcp::spawn(ctx.clone(), addr).await? {
                bound.push((Protocol::Tcp, a));
            }
        }
    }
    if cfg.udp_port != 0 {
        for addr in bind_addrs(&cfg.bind, cfg.udp_port)? {
            if let Some(a) = udp::spawn(ctx.clone(), addr).await? {
                bound.push((Protocol::Udp, a));
            }
        }
    }
    if cfg.ws_port != 0 {
        for addr in bind_addrs(&cfg.bind, cfg.ws_port)? {
            if let Some(a) = ws::spawn(ctx.clone(), addr, false).await? {
                bound.push((Protocol::Ws, a));
            }
        }
    }
    if cfg.wss_port != 0 && ctx.tls.is_some() {
        for addr in bind_addrs(&cfg.bind, cfg.wss_port)? {
            if let Some(a) = ws::spawn(ctx.clone(), addr, true).await? {
                bound.push((Protocol::Wss, a));
            }
        }
    }
    if bound.is_empty() {
        anyhow::bail!("no listeners enabled (all ports are 0)");
    }
    Ok(bound)
}

/// Binding the v6 wildcard can fail with EADDRINUSE on dual-stack kernels after v4 succeeded, or
/// with EAFNOSUPPORT where IPv6 is disabled. Both are expected and skipped, not fatal.
pub(crate) fn tolerable_bind_error(addr: &SocketAddr, e: &std::io::Error) -> bool {
    addr.is_ipv6()
        && matches!(
            e.kind(),
            std::io::ErrorKind::AddrInUse | std::io::ErrorKind::Unsupported
        )
        || (addr.is_ipv6() && e.raw_os_error() == Some(97)) // EAFNOSUPPORT
}
