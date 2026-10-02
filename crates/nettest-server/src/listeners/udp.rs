//! UDP listener: one socket, sessions keyed by peer address, swept on idle timeout.
//!
//! Download runs stream `TpData` from a spawned task through the same shared socket so the recv
//! loop is never blocked by a bulk sender. Datagrams that are not nettest frames get the
//! identification banner, rate-limited (see `banner::BannerLimiter`); valid-looking frames from
//! unknown peers are still dropped so the server cannot be used as a reflector.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nettest_proto::frame::{Frame, HEADER_LEN, Header, Kind};
use nettest_proto::transport::Protocol;
use nettest_proto::transport::udp::MAX_DATAGRAM;
use tokio::net::UdpSocket;

use crate::ServerCtx;
use crate::listeners::banner::{BannerLimiter, banner_message};
use crate::session::SessionId;
use crate::session::echo::{Action, SessionState, process, summarize};

struct UdpSession {
    id: SessionId,
    state: SessionState,
    last_seen: Instant,
}

pub async fn spawn(ctx: Arc<ServerCtx>, addr: SocketAddr) -> anyhow::Result<Option<SocketAddr>> {
    let sock = match UdpSocket::bind(addr).await {
        Ok(s) => s,
        Err(e) if super::tolerable_bind_error(&addr, &e) => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("bind udp {addr}: {e}")),
    };
    let local = sock.local_addr()?;
    ctx.log.info(format!("listening udp://{local}"));
    let sock = Arc::new(sock);

    tokio::spawn(async move {
        let mut sessions: HashMap<SocketAddr, UdpSession> = HashMap::new();
        let mut buf = vec![0u8; MAX_DATAGRAM + HEADER_LEN];
        let idle = Duration::from_secs(ctx.config.idle_timeout_secs.max(1));
        let mut sweep = tokio::time::interval(Duration::from_secs(5));
        let banner = banner_message(&ctx.config, false);
        let mut limiter = BannerLimiter::new(Instant::now());

        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                _ = sweep.tick() => {
                    let now = Instant::now();
                    limiter.sweep(now);
                    let dead: Vec<SocketAddr> = sessions
                        .iter()
                        .filter(|(_, s)| now.duration_since(s.last_seen) > idle)
                        .map(|(a, _)| *a)
                        .collect();
                    for a in dead {
                        if let Some(s) = sessions.remove(&a)
                            && let Some(st) = ctx.sessions.close(s.id)
                        {
                            ctx.log.info(format!("[{}] udp {a} expired (idle {}s); {}", s.id, idle.as_secs(), summarize(&st)));
                        }
                    }
                }
                r = sock.recv_from(&mut buf) => {
                    let (n, peer) = match r {
                        Ok(x) => x,
                        Err(e) => {
                            ctx.log.warn(format!("udp recv error: {e}"));
                            continue;
                        }
                    };
                    let frame = match Frame::parse(&buf[..n]) {
                        Ok(f) => f,
                        Err(_) => {
                            // Not a nettest frame: a scanner or a curious admin. Identify
                            // ourselves, within the reflection budget.
                            if limiter.allow(peer.ip(), Instant::now()) {
                                let _ = sock.send_to(banner.as_bytes(), peer).await;
                                ctx.log.info(format!("[scan] udp {peer}: banner sent"));
                            }
                            continue;
                        }
                    };
                    let is_hello = frame.header.kind == Kind::Hello;
                    let sess = match sessions.get_mut(&peer) {
                        Some(s) => s,
                        None if is_hello => {
                            let id = ctx.sessions.open(peer, Protocol::Udp);
                            ctx.log.info(format!("[{id}] udp hello from {peer}"));
                            sessions.entry(peer).or_insert(UdpSession {
                                id,
                                state: SessionState::default(),
                                last_seen: Instant::now(),
                            })
                        }
                        // Unknown peer sending non-hello frames: silently drop. Replying would
                        // turn the server into a reflector.
                        None => continue,
                    };
                    sess.last_seen = Instant::now();
                    let id = sess.id;
                    match process(&ctx, id, &mut sess.state, frame) {
                        Action::Nothing => {}
                        Action::Reply(reply) => {
                            let bytes = reply.to_bytes();
                            if sock.send_to(&bytes, peer).await.is_ok() {
                                ctx.sessions.update(id, |s| s.bytes_out += bytes.len() as u64);
                            }
                        }
                        Action::Reject(why) => {
                            let f = Frame::new(Header::new(Kind::Error), why.clone().into_bytes());
                            let _ = sock.send_to(&f.to_bytes(), peer).await;
                            sessions.remove(&peer);
                            ctx.sessions.close(id);
                            ctx.log.warn(format!("[{id}] udp {peer} rejected: {why}"));
                        }
                        Action::Download { secs, chunk } => {
                            let sock2 = sock.clone();
                            let ctx2 = ctx.clone();
                            // UDP payloads must fit in a datagram regardless of what was asked.
                            let chunk = chunk.min((MAX_DATAGRAM - HEADER_LEN) as u32);
                            tokio::spawn(async move {
                                let payload = vec![0xA5u8; chunk as usize];
                                let deadline = Instant::now() + Duration::from_secs(secs);
                                let mut seq = 0u64;
                                let mut sent = 0u64;
                                while Instant::now() < deadline && !ctx2.cancel.is_cancelled() {
                                    let f = Frame::new(Header::new(Kind::TpData).with_seq(seq), payload.clone());
                                    if sock2.send_to(&f.to_bytes(), peer).await.is_err() {
                                        break;
                                    }
                                    seq += 1;
                                    sent += HEADER_LEN as u64 + chunk as u64;
                                    // Without pacing a UDP sender will flood the local NIC queue
                                    // and report meaningless numbers; yield between datagrams.
                                    tokio::task::yield_now().await;
                                }
                                let end = Frame::empty(Kind::TpEnd);
                                let mut h = end.header;
                                h.seq = seq;
                                let _ = sock2.send_to(&Frame::new(h, bytes::Bytes::new()).to_bytes(), peer).await;
                                ctx2.sessions.update(id, |s| s.bytes_out += sent);
                                ctx2.log.info(format!("[{id}] udp download done: {seq} datagrams, {:.2} MB", sent as f64 / 1e6));
                            });
                        }
                        Action::Close => {
                            sessions.remove(&peer);
                            if let Some(st) = ctx.sessions.close(id) {
                                ctx.log.info(format!("[{id}] udp {peer} bye; {}", summarize(&st)));
                            }
                        }
                    }
                }
            }
        }
    });
    Ok(Some(local))
}
