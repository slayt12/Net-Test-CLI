//! Protocol handler. `process()` is a pure state machine over frames so TCP/WS (stream) and UDP
//! (datagram) listeners share one implementation; `handle_stream()` drives it for connections.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use nettest_proto::frame::payloads::{
    HelloAckPayload, HelloPayload, TpResultPayload, TpStartPayload, from_json, to_json,
};
use nettest_proto::frame::{Frame, HEADER_LEN, Header, Kind, flags};
use nettest_proto::transport::{AnyTransport, Protocol, TransportError};

use super::auth::token_ok;
use super::{SessionId, SessionStats};
use crate::ServerCtx;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct SessionState {
    pub authed: bool,
    upload: Option<Upload>,
}

struct Upload {
    bytes: u64,
    frames: u64,
    started: Instant,
}

pub enum Action {
    Nothing,
    Reply(Frame),
    /// Send `Error(reason)` then drop the session.
    Reject(String),
    /// Stream `TpData` frames of `chunk` bytes for `secs`, then `TpEnd`.
    Download {
        secs: u64,
        chunk: u32,
    },
    /// Client said goodbye.
    Close,
}

pub fn process(ctx: &ServerCtx, id: SessionId, st: &mut SessionState, frame: Frame) -> Action {
    let now = Instant::now();
    let size = HEADER_LEN as u64 + frame.payload.len() as u64;
    ctx.sessions.update(id, |s| {
        s.last_seen = now;
        s.frames_in += 1;
        s.bytes_in += size;
    });

    if !st.authed {
        return match frame.header.kind {
            Kind::Hello => {
                let hello: HelloPayload = from_json(&frame.payload).unwrap_or_default();
                if !token_ok(&ctx.config.token, hello.token.as_deref()) {
                    return Action::Reject("auth".into());
                }
                st.authed = true;
                ctx.sessions.update(id, |s| {
                    s.authed = true;
                    s.client_id = hello.client_id.clone();
                    s.mode = hello.mode.clone();
                });
                let ack = HelloAckPayload {
                    session_id: id,
                    server_version: crate::version().to_string(),
                };
                Action::Reply(Frame::new(Header::new(Kind::HelloAck), to_json(&ack)))
            }
            _ => Action::Reject("hello required".into()),
        };
    }

    match frame.header.kind {
        Kind::Probe => {
            ctx.sessions.update(id, |s| s.probes += 1);
            let mut h = frame.header;
            h.kind = Kind::Echo;
            Action::Reply(Frame::new(h, frame.payload))
        }
        Kind::Heartbeat => {
            let mut h = frame.header;
            h.kind = Kind::HeartbeatAck;
            Action::Reply(Frame::new(h, Bytes::new()))
        }
        Kind::TpStart => {
            let p: TpStartPayload = from_json(&frame.payload).unwrap_or_default();
            let chunk = p.chunk.clamp(1, ctx.config.max_payload_bytes.max(1));
            let secs = p.secs.clamp(1, 3600);
            if frame.header.flags & flags::TP_DOWNLOAD != 0 {
                Action::Download { secs, chunk }
            } else {
                st.upload = Some(Upload {
                    bytes: 0,
                    frames: 0,
                    started: now,
                });
                Action::Nothing
            }
        }
        Kind::TpData => {
            if let Some(u) = st.upload.as_mut() {
                u.bytes += size;
                u.frames += 1;
            }
            Action::Nothing
        }
        Kind::TpEnd => match st.upload.take() {
            Some(u) => {
                let res = TpResultPayload {
                    bytes: u.bytes,
                    elapsed_ns: now.duration_since(u.started).as_nanos() as u64,
                    frames: u.frames,
                };
                Action::Reply(Frame::new(
                    Header::new(Kind::TpResult).with_seq(frame.header.seq),
                    to_json(&res),
                ))
            }
            None => Action::Nothing,
        },
        Kind::Hello => {
            // Re-hello on an authed session is harmless; ack again.
            let ack = HelloAckPayload {
                session_id: id,
                server_version: crate::version().to_string(),
            };
            Action::Reply(Frame::new(Header::new(Kind::HelloAck), to_json(&ack)))
        }
        Kind::Bye => Action::Close,
        // Server-originated kinds arriving from a client are ignored, not fatal.
        Kind::Echo | Kind::HelloAck | Kind::HeartbeatAck | Kind::TpResult | Kind::Error => {
            Action::Nothing
        }
    }
}

/// Drive a stream transport (TCP / WS / WSS) until it closes, errors, idles out or is cancelled.
pub async fn handle_stream(mut t: AnyTransport, ctx: Arc<ServerCtx>, protocol: Protocol) {
    let peer = t
        .peer_addr()
        .unwrap_or_else(|| "0.0.0.0:0".parse().unwrap());
    let id = ctx.sessions.open(peer, protocol);
    ctx.log
        .info(format!("[{id}] {protocol} connect from {peer}"));
    let mut st = SessionState::default();
    let idle = Duration::from_secs(ctx.config.idle_timeout_secs.max(1));

    let reason = loop {
        let wait = if st.authed { idle } else { HELLO_TIMEOUT };
        let frame = tokio::select! {
            _ = ctx.cancel.cancelled() => break "server shutdown".to_string(),
            r = tokio::time::timeout(wait, t.recv()) => match r {
                Err(_) if st.authed => break format!("idle for {}s", wait.as_secs()),
                Err(_) => break "no hello within timeout".to_string(),
                Ok(Err(TransportError::Closed)) => break "closed by client".to_string(),
                Ok(Err(e)) => break format!("error: {e}"),
                Ok(Ok(f)) => f,
            },
        };

        match process(&ctx, id, &mut st, frame) {
            Action::Nothing => {}
            Action::Reply(reply) => {
                let n = HEADER_LEN as u64 + reply.payload.len() as u64;
                if let Err(e) = t.send_frame(reply).await {
                    break format!("send failed: {e}");
                }
                ctx.sessions.update(id, |s| s.bytes_out += n);
            }
            Action::Reject(why) => {
                let _ = t
                    .send_frame(Frame::new(
                        Header::new(Kind::Error),
                        why.clone().into_bytes(),
                    ))
                    .await;
                let _ = t.close().await;
                break format!("rejected: {why}");
            }
            Action::Download { secs, chunk } => {
                if let Err(e) = stream_download(&mut t, &ctx, id, secs, chunk).await {
                    break format!("download send failed: {e}");
                }
            }
            Action::Close => {
                let _ = t.close().await;
                break "bye".to_string();
            }
        }
    };

    if let Some(s) = ctx.sessions.close(id) {
        ctx.log.info(format!(
            "[{id}] {protocol} {peer} disconnected ({reason}); {}",
            summarize(&s)
        ));
    }
}

pub fn summarize(s: &SessionStats) -> String {
    format!(
        "{} probes, {} frames, {:.1} KB in, {:.1} KB out, {:.0}s",
        s.probes,
        s.frames_in,
        s.bytes_in as f64 / 1e3,
        s.bytes_out as f64 / 1e3,
        s.connected_at.elapsed().as_secs_f64()
    )
}

async fn stream_download(
    t: &mut AnyTransport,
    ctx: &ServerCtx,
    id: SessionId,
    secs: u64,
    chunk: u32,
) -> Result<(), TransportError> {
    let payload = Bytes::from(vec![0xA5u8; chunk as usize]);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seq = 0u64;
    let mut sent = 0u64;
    while Instant::now() < deadline {
        if ctx.cancel.is_cancelled() {
            break;
        }
        t.send(Header::new(Kind::TpData).with_seq(seq), payload.clone())
            .await?;
        seq += 1;
        sent += HEADER_LEN as u64 + chunk as u64;
    }
    t.send(Header::new(Kind::TpEnd).with_seq(seq), Bytes::new())
        .await?;
    ctx.sessions.update(id, |s| s.bytes_out += sent);
    ctx.log.info(format!(
        "[{id}] download done: {seq} frames, {:.2} MB in {secs}s",
        sent as f64 / 1e6
    ));
    Ok(())
}
