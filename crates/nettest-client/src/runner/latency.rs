//! Latency / soak loop: sequenced probes (or heartbeats) at a fixed interval, loss by timeout,
//! automatic reconnect with backoff. Soak mode is the same loop with a slower cadence and the
//! connection-stability log promoted into the summary.

use std::time::{Duration, Instant};

use bytes::Bytes;
use nettest_proto::config::TestMode;
use nettest_proto::frame::{HEADER_LEN, Header, Kind};
use nettest_proto::sinks::{EventKind, LogLevel};
use nettest_proto::stats::{ChartPoint, LatencyTracker, LossPolicy, RunSummary, SoakLog};
use nettest_proto::time::now_ns;
use nettest_proto::transport::{AnyTransport, TransportError, dial};

use super::pacing::{policy_timeout as tracker_policy_timeout, summary};
use super::{Backoff, Runner, Snapshot, StopReason, UiEvent, filler};

const SNAPSHOT_EVERY: Duration = Duration::from_millis(250);

pub async fn run(r: &mut Runner) -> (RunSummary, StopReason, Vec<ChartPoint>) {
    let soak = r.cfg.mode == TestMode::Soak;
    let interval = Duration::from_millis(if soak {
        r.cfg.heartbeat_ms.max(100)
    } else {
        r.cfg.interval_ms.max(1)
    });
    let probe_kind = if soak { Kind::Heartbeat } else { Kind::Probe };
    let reply_kind = if soak { Kind::HeartbeatAck } else { Kind::Echo };
    let payload_len = (r.cfg.payload_bytes as usize).saturating_sub(HEADER_LEN);
    let payload: Bytes = if soak {
        Bytes::new()
    } else {
        filler(payload_len, 7).into()
    };

    let loss_override = r.cfg.loss_timeout_ms;
    let mut tracker = LatencyTracker::new(LossPolicy::resolve(interval, loss_override), 4096);
    let mut soak_log = SoakLog::with_capacity(r.history_limit().max(64));
    let deadline = (r.cfg.duration_secs > 0)
        .then(|| Instant::now() + Duration::from_secs(r.cfg.duration_secs));
    let max_count = r.cfg.count;

    let opts = match r.dial_options() {
        Ok(o) => o,
        Err(e) => {
            r.emit_message(LogLevel::Error, e.clone());
            return (
                summary(&tracker, soak.then(|| soak_log.snapshot())),
                StopReason::ConnectFailed(e),
                tracker.chart_points(),
            );
        }
    };

    let mut transport: Option<AnyTransport> = match dial(&opts).await {
        Ok((t, timings)) => {
            r.on_connected(&timings);
            soak_log.on_connected(Instant::now());
            Some(t)
        }
        Err(e) => {
            let msg = e.to_string();
            r.emit_message(LogLevel::Error, format!("connect failed: {msg}"));
            return (
                summary(&tracker, soak.then(|| soak_log.snapshot())),
                StopReason::ConnectFailed(msg),
                tracker.chart_points(),
            );
        }
    };

    let mut seq: u64 = 0;
    let mut backoff = Backoff::new();
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut housekeeping = tokio::time::interval(SNAPSHOT_EVERY);
    let mut reconnect_at: Option<Instant> = None;
    // After a bounded run sends its last probe we still wait for echoes before declaring done.
    let mut draining_until: Option<Instant> = None;

    let reason = loop {
        if let Some(d) = deadline
            && Instant::now() >= d
            && draining_until.is_none()
        {
            draining_until = Some(Instant::now() + tracker_policy_timeout(interval, loss_override));
        }
        if let Some(until) = draining_until
            && (Instant::now() >= until || tracker.snapshot().in_flight == 0)
        {
            break StopReason::Completed;
        }

        tokio::select! {
            cmd = r.commands.recv() => {
                if cmd.is_none() || matches!(cmd, Some(super::Command::Stop)) {
                    break StopReason::Stopped;
                }
            }

            _ = tick.tick(), if transport.is_some() && draining_until.is_none() => {
                if max_count > 0 && seq >= max_count {
                    draining_until = Some(Instant::now() + tracker_policy_timeout(interval, loss_override));
                    continue;
                }
                seq += 1;
                let now = Instant::now();
                let header = Header::new(probe_kind).with_seq(seq).with_send_ns(now_ns());
                tracker.on_sent(seq, now);
                if let Some(t) = transport.as_mut()
                    && let Err(e) = t.send(header, payload.clone()).await
                {
                    disconnect(r, &mut transport, &mut soak_log, &mut backoff, &mut reconnect_at, &format!("send: {e}"));
                }
            }

            res = async { transport.as_mut().unwrap().recv().await }, if transport.is_some() => {
                match res {
                    Ok(f) if f.header.kind == reply_kind => {
                        let rtt_ns = now_ns().saturating_sub(f.header.client_send_ns);
                        let s = tracker.on_echo(f.header.seq, Duration::from_nanos(rtt_ns), Instant::now());
                        let snap = tracker.snapshot();
                        r.on_sample(&s, &snap);
                    }
                    Ok(f) if f.header.kind == Kind::Error => {
                        let why = String::from_utf8_lossy(&f.payload).into_owned();
                        disconnect(r, &mut transport, &mut soak_log, &mut backoff, &mut reconnect_at, &format!("server error: {why}"));
                    }
                    Ok(_) => {}
                    Err(TransportError::Closed) => {
                        disconnect(r, &mut transport, &mut soak_log, &mut backoff, &mut reconnect_at, "closed by peer");
                    }
                    Err(e) => {
                        disconnect(r, &mut transport, &mut soak_log, &mut backoff, &mut reconnect_at, &e.to_string());
                    }
                }
            }

            _ = async { tokio::time::sleep_until(reconnect_at.unwrap().into()).await }, if transport.is_none() && reconnect_at.is_some() => {
                soak_log.on_reconnect_attempt();
                match dial(&opts).await {
                    Ok((t, timings)) => {
                        transport = Some(t);
                        reconnect_at = None;
                        backoff.reset();
                        soak_log.on_connected(Instant::now());
                        r.on_connected(&timings);
                        tick.reset();
                    }
                    Err(e) => {
                        let wait = backoff.next();
                        r.emit_log(LogLevel::Warn, EventKind::Reconnecting { attempt: backoff.attempt, backoff_ms: wait.as_millis() as u64 });
                        r.emit_message(LogLevel::Debug, format!("reconnect attempt {} failed: {e}", backoff.attempt));
                        let _ = r.events.try_send(UiEvent::Reconnecting { attempt: backoff.attempt, backoff_ms: wait.as_millis() as u64, error: Some(e.to_string()) });
                        reconnect_at = Some(Instant::now() + wait);
                    }
                }
            }

            _ = housekeeping.tick() => {
                for lost in tracker.expire(Instant::now()) {
                    let snap = tracker.snapshot();
                    r.on_sample(&lost, &snap);
                }
                let snap = Snapshot {
                    latency: tracker.snapshot(),
                    throughput: None,
                    soak: Some(soak_log.snapshot()),
                    chart: tracker.chart_points(),
                    connected: transport.is_some(),
                    elapsed_s: r.elapsed_s(),
                };
                let _ = r.events.try_send(UiEvent::Snapshot(Box::new(snap)));
            }
        }
    };

    if let Some(mut t) = transport.take() {
        let _ = t.send(Header::new(Kind::Bye), Bytes::new()).await;
        let _ = t.close().await;
    }
    for lost in tracker.drain() {
        let snap = tracker.snapshot();
        r.on_sample(&lost, &snap);
    }
    let chart = tracker.chart_points();
    (
        summary(&tracker, soak.then(|| soak_log.snapshot())),
        reason,
        chart,
    )
}

fn disconnect(
    r: &mut Runner,
    transport: &mut Option<AnyTransport>,
    soak_log: &mut SoakLog,
    backoff: &mut Backoff,
    reconnect_at: &mut Option<Instant>,
    reason: &str,
) {
    if transport.take().is_none() {
        return;
    }
    soak_log.on_disconnected(Instant::now(), reason);
    r.on_disconnected(reason);
    let wait = backoff.next();
    r.emit_log(
        LogLevel::Warn,
        EventKind::Reconnecting {
            attempt: backoff.attempt,
            backoff_ms: wait.as_millis() as u64,
        },
    );
    let _ = r.events.try_send(UiEvent::Reconnecting {
        attempt: backoff.attempt,
        backoff_ms: wait.as_millis() as u64,
        error: None,
    });
    *reconnect_at = Some(Instant::now() + wait);
}
