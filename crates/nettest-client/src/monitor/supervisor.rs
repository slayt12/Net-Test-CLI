//! One task per monitored target: owns a `Runner`, normalises its `UiEvent`s into health
//! signals, and restarts it when it exits (a target that is down at start, or DNS that needs
//! re-resolving).
//!
//! Why reuse the runner instead of a simpler "connect once per check": the existing loops
//! already give a persistent connection with reconnect for the nettest protocols and
//! concurrent, timeout-bounded probes for the serverless ones, with loss detection on both.
//! The runner treats a failed first dial as fatal, which is right for a test and wrong for a
//! monitor; the supervisor turns that into a failure signal plus a backoff-paced respawn.
//!
//! Invariant: for serverless targets `Connected` is *not* a success (the runner emits it right
//! after DNS, before any probe); only samples decide.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nettest_proto::log::LineLog;
use nettest_proto::sinks::MultiSink;
use nettest_proto::stats::{LatencySnapshot, SampleStatus};
use nettest_proto::transport::Protocol;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::config::ResolvedTarget;
use super::fmt_duration;
use super::health::{Health, Signal, State, Transition};
use crate::runner::{Backoff, Command, Runner, StopReason, UiEvent};

#[derive(Debug, Clone)]
pub struct TargetEvent {
    pub index: usize,
    pub name: String,
    /// `scheme://host:port` for the alert text.
    pub target: String,
    pub at: chrono::DateTime<chrono::Local>,
    pub transition: Transition,
}

/// Shared with the periodic summary.
#[derive(Debug, Clone, Default)]
pub struct TargetStatus {
    pub state: State,
    /// When the current state began.
    pub since: Option<Instant>,
    pub last_reason: String,
    pub last_snapshot: Option<LatencySnapshot>,
}

/// Map a runner event to a health signal, or `None` when it says nothing about reachability.
pub fn signal_of(protocol: Protocol, ev: &UiEvent) -> Option<Signal> {
    let serverless = protocol.is_serverless();
    match ev {
        UiEvent::Sample(s) => match s.status {
            SampleStatus::Ok | SampleStatus::Late => Some(Signal::Success),
            SampleStatus::Lost => Some(Signal::Failure("probe lost (no reply)".into())),
            SampleStatus::Failed => Some(Signal::Failure(
                s.detail.clone().unwrap_or_else(|| "probe failed".into()),
            )),
            SampleStatus::Duplicate | SampleStatus::OutOfOrder => None,
        },
        UiEvent::Connected(_) if !serverless => Some(Signal::Success),
        UiEvent::Disconnected(r) if !serverless => Some(Signal::Failure(r.clone())),
        // Attempt 1 is the scheduling notice that accompanies `Disconnected`; later ones are
        // reconnects that actually failed.
        UiEvent::Reconnecting { attempt, error, .. } if !serverless && *attempt >= 2 => {
            Some(Signal::Failure(match error {
                Some(e) => format!("reconnect attempt {attempt} failed: {e}"),
                None => format!("reconnect attempt {attempt} failed"),
            }))
        }
        _ => None,
    }
}

pub async fn run_target(
    index: usize,
    t: ResolvedTarget,
    events: mpsc::Sender<TargetEvent>,
    status: Arc<Mutex<TargetStatus>>,
    log: LineLog,
    cancel: CancellationToken,
) {
    let name = t.name.clone();
    let target = t.client.target();
    let serverless = t.client.protocol.is_serverless();
    let mut health = Health::new(t.thresholds);
    let mut backoff = Backoff::new();
    let mut setup_failures: u32 = 0;

    let emit = |tr: Transition, health: &Health| {
        let now = Instant::now();
        if let Ok(mut st) = status.lock() {
            st.state = health.state();
            st.since = Some(match tr {
                Transition::Down { .. } | Transition::Initial { up: false, .. } => now,
                Transition::Up { .. } | Transition::Initial { up: true, .. } => now,
                Transition::Remind { .. } => st.since.unwrap_or(now),
            });
            st.last_reason = health.last_reason().to_string();
        }
        match &tr {
            Transition::Initial { up: true, .. } => log.info(format!("[{name}] up")),
            Transition::Initial { up: false, reason } => {
                log.warn(format!("[{name}] DOWN at start: {reason}"))
            }
            Transition::Down { reason, consecutive } => log.warn(format!(
                "[{name}] DOWN after {consecutive} consecutive failures: {reason}"
            )),
            Transition::Up { downtime, .. } => {
                log.info(format!("[{name}] UP again after {}", fmt_duration(*downtime)))
            }
            Transition::Remind { downtime, reason } => log.warn(format!(
                "[{name}] still DOWN for {}: {reason}",
                fmt_duration(*downtime)
            )),
        }
        let ev = TargetEvent {
            index,
            name: name.clone(),
            target: target.clone(),
            at: chrono::Local::now(),
            transition: tr,
        };
        if events.try_send(ev).is_err() {
            log.error(format!("[{name}] event queue full; transition not delivered"));
        }
    };

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let (ev_tx, mut ev_rx) = mpsc::channel::<UiEvent>(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(4);
        let mut runner = Runner::new(t.client.clone(), MultiSink::new(), ev_tx, cmd_rx);
        runner.set_history_limit(0);
        let handle = tokio::spawn(runner.run());
        let mut restart_requested = false;
        let mut stop_sent = false;

        let output = loop {
            let reminder = health.next_reminder();
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = cmd_tx.send(Command::Stop).await;
                    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
                    return;
                }
                _ = async { tokio::time::sleep_until(reminder.unwrap().into()).await }, if reminder.is_some() => {
                    if let Some(tr) = health.on_tick(Instant::now()) {
                        emit(tr, &health);
                    }
                }
                ev = ev_rx.recv() => match ev {
                    None | Some(UiEvent::Finished(_)) => break handle.await,
                    Some(UiEvent::Snapshot(s)) => {
                        if let Ok(mut st) = status.lock() {
                            st.last_snapshot = Some(s.latency);
                        }
                    }
                    Some(ev) => {
                        let Some(sig) = signal_of(t.client.protocol, &ev) else { continue };
                        if sig == Signal::Success {
                            backoff.reset();
                            setup_failures = 0;
                        }
                        if let Some(tr) = health.on_signal(sig, Instant::now()) {
                            let went_down = matches!(
                                tr,
                                Transition::Down { .. } | Transition::Initial { up: false, .. }
                            );
                            emit(tr, &health);
                            // A serverless prober resolved DNS once at start; a fresh one picks up
                            // an address change and recreates its socket.
                            if went_down && serverless && !stop_sent {
                                restart_requested = true;
                                stop_sent = true;
                                let _ = cmd_tx.send(Command::Stop).await;
                            }
                        }
                    }
                },
            }
        };

        let wait = match output {
            Ok(out) => match out.reason {
                StopReason::ConnectFailed(e) => {
                    setup_failures += 1;
                    let wait = backoff.next();
                    // The first failure and then every tenth: enough for the log, not a flood.
                    if setup_failures == 1 || setup_failures.is_multiple_of(10) {
                        log.warn(format!(
                            "[{name}] cannot start probing ({e}); attempt {setup_failures}, retrying in {}",
                            fmt_duration(wait)
                        ));
                    }
                    if let Some(tr) = health.on_signal(Signal::Failure(e), Instant::now()) {
                        emit(tr, &health);
                    }
                    wait
                }
                _ if restart_requested => Duration::from_millis(100),
                other => {
                    log.warn(format!(
                        "[{name}] probe loop ended unexpectedly ({}); restarting",
                        other.describe()
                    ));
                    Duration::from_secs(1)
                }
            },
            Err(e) => {
                log.error(format!("[{name}] probe task panicked: {e}; restarting"));
                backoff.next()
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nettest_proto::stats::Sample;
    use nettest_proto::transport::ConnectTimings;

    fn sample(status: SampleStatus, detail: Option<&str>) -> UiEvent {
        UiEvent::Sample(Sample {
            seq: 1,
            sent_at: chrono::Local::now(),
            elapsed_s: 0.0,
            rtt_ms: None,
            status,
            detail: detail.map(str::to_string),
        })
    }

    #[test]
    fn signal_table() {
        for p in [Protocol::Ws, Protocol::Ping] {
            assert_eq!(signal_of(p, &sample(SampleStatus::Ok, None)), Some(Signal::Success));
            assert_eq!(signal_of(p, &sample(SampleStatus::Late, None)), Some(Signal::Success));
            assert!(matches!(signal_of(p, &sample(SampleStatus::Lost, None)), Some(Signal::Failure(_))));
            assert_eq!(
                signal_of(p, &sample(SampleStatus::Failed, Some("connection refused"))),
                Some(Signal::Failure("connection refused".into()))
            );
            assert_eq!(signal_of(p, &sample(SampleStatus::Duplicate, None)), None);
            assert_eq!(signal_of(p, &sample(SampleStatus::OutOfOrder, None)), None);
        }
        let connected = UiEvent::Connected(ConnectTimings::default());
        assert_eq!(signal_of(Protocol::Ws, &connected), Some(Signal::Success));
        assert_eq!(signal_of(Protocol::Ping, &connected), None);
        let dc = UiEvent::Disconnected("closed by peer".into());
        assert_eq!(signal_of(Protocol::Tcp, &dc), Some(Signal::Failure("closed by peer".into())));
        assert_eq!(signal_of(Protocol::Sip, &dc), None);
        let r1 = UiEvent::Reconnecting { attempt: 1, backoff_ms: 500, error: None };
        let r2 = UiEvent::Reconnecting { attempt: 2, backoff_ms: 1000, error: Some("refused".into()) };
        assert_eq!(signal_of(Protocol::Udp, &r1), None);
        assert_eq!(
            signal_of(Protocol::Udp, &r2),
            Some(Signal::Failure("reconnect attempt 2 failed: refused".into()))
        );
        assert_eq!(signal_of(Protocol::Http, &r2), None);
    }
}
