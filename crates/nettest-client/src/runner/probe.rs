//! Serverless loop: one spawned probe per sequence number against a device with no
//! nettest-server (ping / connect / sip / http / https).
//!
//! Differences from `latency.rs`: there is no connection to lose, so "stability" is derived from
//! the probes themselves. `outage_after` consecutive failures open an outage in the `SoakLog`;
//! the next success closes it. Probes run concurrently so a slow or dead target never delays the
//! cadence, and the per-probe timeout equals the tracker's loss timeout so both agree on "lost".

use std::time::{Duration, Instant};

use nettest_proto::config::TestMode;
use nettest_proto::probe::{AnyProber, ProbeError, ProbeReply, ProbeTarget};
use nettest_proto::sinks::LogLevel;
use nettest_proto::stats::{ChartPoint, LatencyTracker, LossPolicy, RunSummary, SoakLog};
use nettest_proto::tls::client::TlsClientMode;
use tokio::task::JoinSet;

use super::pacing::{policy_timeout, summary};
use super::{Runner, Snapshot, StopReason, UiEvent};

const SNAPSHOT_EVERY: Duration = Duration::from_millis(250);

pub async fn run(r: &mut Runner) -> (RunSummary, StopReason, Vec<ChartPoint>) {
    let soak = r.cfg.mode == TestMode::Soak;
    let interval = Duration::from_millis(if soak {
        r.cfg.heartbeat_ms.max(100)
    } else {
        r.cfg.interval_ms.max(1)
    });
    let policy = LossPolicy::resolve(interval, r.cfg.loss_timeout_ms);
    let loss_override = r.cfg.loss_timeout_ms;
    let mut tracker = LatencyTracker::new(policy, 4096);
    let mut soak_log = SoakLog::with_capacity(r.history_limit().max(64));
    let deadline = (r.cfg.duration_secs > 0)
        .then(|| Instant::now() + Duration::from_secs(r.cfg.duration_secs));
    let max_count = r.cfg.count;
    let outage_after = r.cfg.outage_after.max(1);

    let target = match probe_target(r, policy.timeout) {
        Ok(t) => t,
        Err(e) => {
            r.emit_message(LogLevel::Error, e.clone());
            return (
                summary(&tracker, soak.then(|| soak_log.snapshot())),
                StopReason::ConnectFailed(e),
                tracker.chart_points(),
            );
        }
    };
    let (prober, timings) = match AnyProber::open(&target).await {
        Ok(x) => x,
        Err(e) => {
            let msg = e.to_string();
            r.emit_message(LogLevel::Error, format!("probe setup failed: {msg}"));
            return (
                summary(&tracker, soak.then(|| soak_log.snapshot())),
                StopReason::ConnectFailed(msg),
                tracker.chart_points(),
            );
        }
    };
    r.emit_message(LogLevel::Info, prober.describe());
    r.on_connected(&timings);
    soak_log.on_connected(Instant::now());

    let mut seq: u64 = 0;
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut housekeeping = tokio::time::interval(SNAPSHOT_EVERY);
    let mut inflight: JoinSet<(u64, Result<ProbeReply, ProbeError>)> = JoinSet::new();
    let mut state = Stability {
        consecutive_failures: 0,
        outage: false,
        outage_after,
    };
    let mut draining_until: Option<Instant> = None;

    let reason = loop {
        if let Some(d) = deadline
            && Instant::now() >= d
            && draining_until.is_none()
        {
            draining_until = Some(Instant::now() + policy_timeout(interval, loss_override));
        }
        if let Some(until) = draining_until
            && (Instant::now() >= until || inflight.is_empty())
        {
            break StopReason::Completed;
        }

        tokio::select! {
            cmd = r.commands.recv() => {
                if cmd.is_none() || matches!(cmd, Some(super::Command::Stop)) {
                    break StopReason::Stopped;
                }
            }

            _ = tick.tick(), if draining_until.is_none() => {
                if max_count > 0 && seq >= max_count {
                    draining_until = Some(Instant::now() + policy_timeout(interval, loss_override));
                    continue;
                }
                seq += 1;
                tracker.on_sent(seq, Instant::now());
                let p = prober.clone();
                inflight.spawn(async move { (seq, p.probe(seq).await) });
            }

            Some(joined) = inflight.join_next(), if !inflight.is_empty() => {
                let Ok((seq, res)) = joined else { continue };
                match res {
                    Ok(reply) => {
                        let mut s = tracker.on_echo(seq, reply.rtt, Instant::now());
                        s.detail = reply.detail;
                        let snap = tracker.snapshot();
                        r.on_sample(&s, &snap);
                        if state.on_success() {
                            soak_log.on_connected(Instant::now());
                            r.emit_message(LogLevel::Info, "target answering again");
                            r.on_connected(&timings);
                        }
                    }
                    Err(ProbeError::Timeout) => {
                        // The tracker owns the Lost sample; its timeout matches the probe's.
                        for lost in tracker.expire(Instant::now()) {
                            let snap = tracker.snapshot();
                            r.on_sample(&lost, &snap);
                        }
                        note_failure(r, &mut state, &mut soak_log, "probes timing out");
                    }
                    Err(e) => {
                        let s = tracker.on_failed(seq, Instant::now(), e.to_string());
                        let snap = tracker.snapshot();
                        r.on_sample(&s, &snap);
                        note_failure(r, &mut state, &mut soak_log, &e.to_string());
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
                    connected: !state.outage,
                    elapsed_s: r.elapsed_s(),
                };
                let _ = r.events.try_send(UiEvent::Snapshot(Box::new(snap)));
            }
        }
    };

    inflight.abort_all();
    drop(prober);
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

struct Stability {
    consecutive_failures: u32,
    outage: bool,
    outage_after: u32,
}

impl Stability {
    /// Returns true when this success closes an outage.
    fn on_success(&mut self) -> bool {
        self.consecutive_failures = 0;
        std::mem::replace(&mut self.outage, false)
    }

    /// Returns true when this failure opens an outage.
    fn on_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        if !self.outage && self.consecutive_failures >= self.outage_after {
            self.outage = true;
            return true;
        }
        false
    }
}

fn note_failure(r: &mut Runner, state: &mut Stability, soak_log: &mut SoakLog, why: &str) {
    if state.on_failure() {
        let reason = format!(
            "{} consecutive failures ({why})",
            state.consecutive_failures
        );
        soak_log.on_disconnected(Instant::now(), reason.clone());
        r.on_disconnected(&reason);
    }
}

fn probe_target(r: &Runner, timeout: Duration) -> Result<ProbeTarget, String> {
    let tls = if !r.cfg.fingerprint.trim().is_empty() {
        Some(TlsClientMode::Pinned(
            nettest_proto::tls::fingerprint::parse(&r.cfg.fingerprint)?,
        ))
    } else {
        None
    };
    Ok(ProbeTarget {
        protocol: r.cfg.protocol,
        host: r.cfg.host.clone(),
        port: r.cfg.port,
        prefer_ipv6: r.cfg.ipv6,
        bind: None,
        timeout,
        payload_bytes: r.cfg.payload_bytes as usize,
        path: r.cfg.ws_path.clone(),
        tls,
    })
}

#[cfg(test)]
mod tests {
    use super::Stability;

    #[test]
    fn outage_opens_and_closes_once() {
        let mut s = Stability {
            consecutive_failures: 0,
            outage: false,
            outage_after: 3,
        };
        assert!(!s.on_failure());
        assert!(!s.on_failure());
        assert!(s.on_failure());
        assert!(!s.on_failure());
        assert!(s.on_success());
        assert!(!s.on_success());
    }
}
