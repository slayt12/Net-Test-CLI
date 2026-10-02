//! Throughput mode: bulk upload (client -> server, server reports what landed) and/or bulk
//! download (server -> client, client counts). Results are in Mbit/s of frame bytes on the wire.

use std::time::{Duration, Instant};

use bytes::Bytes;
use nettest_proto::frame::payloads::{TpResultPayload, TpStartPayload, from_json, to_json};
use nettest_proto::frame::{HEADER_LEN, Header, Kind, flags};
use nettest_proto::sinks::{EventKind, LogLevel};
use nettest_proto::stats::{ChartPoint, RunSummary, ThroughputTracker, TpDirection};
use nettest_proto::transport::udp::DEFAULT_MAX_DATAGRAM;
use nettest_proto::transport::{AnyTransport, Protocol, dial};

use super::{Runner, Snapshot, StopReason, UiEvent, filler};

const RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_EVERY: Duration = Duration::from_millis(200);

pub async fn run(r: &mut Runner) -> (RunSummary, StopReason, Vec<ChartPoint>) {
    let mut tp = ThroughputTracker::new();
    let fail = |tp: &ThroughputTracker, e: String| {
        (
            RunSummary {
                throughput: Some(tp.snapshot()),
                ..Default::default()
            },
            StopReason::ConnectFailed(e),
            Vec::new(),
        )
    };

    let opts = match r.dial_options() {
        Ok(o) => o,
        Err(e) => {
            r.emit_message(LogLevel::Error, e.clone());
            return fail(&tp, e);
        }
    };
    let mut t = match dial(&opts).await {
        Ok((t, timings)) => {
            r.on_connected(&timings);
            t
        }
        Err(e) => {
            r.emit_message(LogLevel::Error, format!("connect failed: {e}"));
            return fail(&tp, e.to_string());
        }
    };

    let secs = r.cfg.tp_secs.max(1);
    let mut chunk = r.cfg.tp_chunk_bytes.max(1);
    if r.cfg.protocol == Protocol::Udp {
        chunk = chunk.min((DEFAULT_MAX_DATAGRAM - HEADER_LEN) as u32);
    }
    let dirs: &[TpDirection] = match r.cfg.tp_direction {
        TpDirection::Upload => &[TpDirection::Upload],
        TpDirection::Download => &[TpDirection::Download],
        TpDirection::Both => &[TpDirection::Upload, TpDirection::Download],
    };

    let mut reason = StopReason::Completed;
    'dirs: for dir in dirs {
        let res = match dir {
            TpDirection::Download => download(r, &mut t, &mut tp, secs, chunk).await,
            _ => upload(r, &mut t, &mut tp, secs, chunk).await,
        };
        match res {
            Ok(true) => {}
            Ok(false) => {
                reason = StopReason::Stopped;
                break 'dirs;
            }
            Err(e) => {
                r.emit_message(LogLevel::Error, format!("{dir}: {e}"));
                reason = StopReason::ConnectFailed(e);
                break 'dirs;
            }
        }
    }

    let _ = t.send(Header::new(Kind::Bye), Bytes::new()).await;
    let _ = t.close().await;
    send_snapshot(r, &tp, false);
    (
        RunSummary {
            throughput: Some(tp.snapshot()),
            ..Default::default()
        },
        reason,
        Vec::new(),
    )
}

fn send_snapshot(r: &mut Runner, tp: &ThroughputTracker, connected: bool) {
    let _ = r.events.try_send(UiEvent::Snapshot(Box::new(Snapshot {
        throughput: Some(tp.snapshot()),
        connected,
        elapsed_s: r.elapsed_s(),
        ..Default::default()
    })));
}

/// Returns Ok(false) if the user stopped the test.
async fn upload(
    r: &mut Runner,
    t: &mut AnyTransport,
    tp: &mut ThroughputTracker,
    secs: u64,
    chunk: u32,
) -> Result<bool, String> {
    r.emit_message(
        LogLevel::Info,
        format!("upload: streaming {chunk}-byte frames for {secs}s"),
    );
    let start = TpStartPayload { secs, chunk };
    t.send(Header::new(Kind::TpStart), to_json(&start).into())
        .await
        .map_err(|e| e.to_string())?;
    let payload: Bytes = filler(chunk as usize, 11).into();
    let now = Instant::now();
    tp.start(TpDirection::Upload, now);
    let deadline = now + Duration::from_secs(secs);
    let mut seq = 0u64;
    let mut last_snap = Instant::now();
    while Instant::now() < deadline {
        if r.stop_requested() {
            return Ok(false);
        }
        t.send(Header::new(Kind::TpData).with_seq(seq), payload.clone())
            .await
            .map_err(|e| e.to_string())?;
        seq += 1;
        tp.add(
            TpDirection::Upload,
            HEADER_LEN as u64 + chunk as u64,
            Instant::now(),
        );
        if last_snap.elapsed() >= SNAPSHOT_EVERY {
            send_snapshot(r, tp, true);
            last_snap = Instant::now();
        }
        // UDP has no backpressure; without a yield the sender outruns the NIC and the kernel
        // drops locally, which would be reported as "path loss".
        if t.protocol() == Protocol::Udp {
            tokio::task::yield_now().await;
        }
    }
    t.send(Header::new(Kind::TpEnd).with_seq(seq), Bytes::new())
        .await
        .map_err(|e| e.to_string())?;
    tp.finish(TpDirection::Upload, Instant::now(), 0);

    // Wait for the server's count of what actually arrived.
    let result = tokio::time::timeout(RESULT_TIMEOUT, async {
        loop {
            match t.recv().await {
                Ok(f) if f.header.kind == Kind::TpResult => {
                    return Ok::<_, String>(
                        from_json::<TpResultPayload>(&f.payload).unwrap_or_default(),
                    );
                }
                Ok(_) => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for server throughput result".to_string())??;
    tp.set_upload_result(
        result.bytes,
        Duration::from_nanos(result.elapsed_ns),
        result.frames,
    );
    let snap = tp.snapshot();
    r.emit_log(
        LogLevel::Info,
        EventKind::ThroughputResult {
            direction: "upload".into(),
            bytes: snap.up_bytes,
            secs: snap.up_secs,
            mbps: snap.up_mbps,
        },
    );
    if seq > result.frames {
        r.emit_message(
            LogLevel::Warn,
            format!(
                "upload: sent {seq} frames, server received {} ({:.2}% lost)",
                result.frames,
                (seq - result.frames) as f64 * 100.0 / seq as f64
            ),
        );
    }
    send_snapshot(r, tp, true);
    Ok(true)
}

async fn download(
    r: &mut Runner,
    t: &mut AnyTransport,
    tp: &mut ThroughputTracker,
    secs: u64,
    chunk: u32,
) -> Result<bool, String> {
    r.emit_message(
        LogLevel::Info,
        format!("download: requesting {chunk}-byte frames for {secs}s"),
    );
    let start = TpStartPayload { secs, chunk };
    t.send(
        Header::new(Kind::TpStart).with_flags(flags::TP_DOWNLOAD),
        to_json(&start).into(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut started = false;
    let mut last_snap = Instant::now();
    let overall = Duration::from_secs(secs) + RESULT_TIMEOUT;
    let deadline = Instant::now() + overall;
    loop {
        if r.stop_requested() {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("download did not finish in time".into());
        }
        let f =
            match tokio::time::timeout(remaining.min(Duration::from_millis(500)), t.recv()).await {
                Ok(Ok(f)) => f,
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => {
                    if last_snap.elapsed() >= SNAPSHOT_EVERY {
                        send_snapshot(r, tp, true);
                        last_snap = Instant::now();
                    }
                    continue;
                }
            };
        match f.header.kind {
            Kind::TpData => {
                let now = Instant::now();
                if !started {
                    tp.start(TpDirection::Download, now);
                    started = true;
                }
                tp.add(
                    TpDirection::Download,
                    HEADER_LEN as u64 + f.payload.len() as u64,
                    now,
                );
                if last_snap.elapsed() >= SNAPSHOT_EVERY {
                    send_snapshot(r, tp, true);
                    last_snap = Instant::now();
                }
            }
            Kind::TpEnd => {
                if !started {
                    tp.start(TpDirection::Download, Instant::now());
                }
                tp.finish(TpDirection::Download, Instant::now(), f.header.seq);
                let snap = tp.snapshot();
                r.emit_log(
                    LogLevel::Info,
                    EventKind::ThroughputResult {
                        direction: "download".into(),
                        bytes: snap.down_bytes,
                        secs: snap.down_secs,
                        mbps: snap.down_mbps,
                    },
                );
                if snap.down_expected_frames > snap.down_frames {
                    r.emit_message(
                        LogLevel::Warn,
                        format!(
                            "download: server sent {} frames, received {} ({:.2}% lost)",
                            snap.down_expected_frames, snap.down_frames, snap.down_loss_pct
                        ),
                    );
                }
                send_snapshot(r, tp, true);
                return Ok(true);
            }
            Kind::Error => {
                return Err(String::from_utf8_lossy(&f.payload).into_owned());
            }
            _ => {}
        }
    }
}
