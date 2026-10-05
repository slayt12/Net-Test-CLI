//! `nettest-client monitor`: watch several endpoints for as long as the process lives and raise
//! alerts (ntfy / Slack / Discord) when one goes DOWN or comes back UP.
//!
//! Why a separate mode: the TUI and headless runs are *tests* with a beginning and an end; a
//! monitor is a service that must survive a target that is down at start, keep bounded memory
//! for months, and turn the runner's stream of samples and disconnects into a handful of
//! state transitions. It reuses the `Runner` per target (`supervisor`), feeds a pure state
//! machine (`health`) and hands transitions to a delivery task (`notify`).
//!
//! Invariants:
//! - Monitoring never blocks on a webhook: deliveries go through a bounded queue and a single
//!   delivery task; a slow or dead webhook endpoint costs log lines, not probes.
//! - Transitions are decided only by `health::Health`; the supervisor merely normalises events.
//! - Every alert is also a log line, so the log alone tells the story when webhooks fail.

pub mod cli;
pub mod config;
pub mod health;
pub mod notify;
pub mod supervisor;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nettest_proto::log::{LineLog, LogOptions};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use config::Resolved;
use health::State;
use supervisor::{TargetEvent, TargetStatus};

pub struct RunOptions {
    pub stderr: bool,
    pub log_file: Option<PathBuf>,
    pub jsonl_file: Option<PathBuf>,
}

/// Run until `cancel` fires. Returns an exit code: 0, or `exit::IO` when the log file cannot be
/// opened.
pub async fn run(resolved: Resolved, opts: RunOptions, cancel: CancellationToken) -> u8 {
    let (log, _tail) = match LineLog::start(LogOptions {
        stderr: opts.stderr,
        text_file: opts.log_file.as_ref().map(|p| p.display().to_string()),
        jsonl_file: opts.jsonl_file.as_ref().map(|p| p.display().to_string()),
        tail_capacity: 1,
    }) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("could not open log file: {e}");
            return crate::cli::exit::IO;
        }
    };
    log.info(format!(
        "nettest-client {} monitor starting on {}: {} target(s), {} notifier(s)",
        env!("CARGO_PKG_VERSION"),
        resolved.hostname,
        resolved.targets.len(),
        resolved.notifiers.len()
    ));
    for w in &resolved.warnings {
        log.warn(w.clone());
    }
    for t in &resolved.targets {
        log.info(format!(
            "target {}: {} every {} (timeout {}, down after {} failures, up after {} successes{})",
            t.name,
            t.client.target(),
            fmt_duration(t.interval),
            fmt_duration(t.timeout),
            t.thresholds.down_after,
            t.thresholds.up_after,
            t.thresholds
                .remind_every
                .map(|d| format!(", remind every {}", fmt_duration(d)))
                .unwrap_or_default()
        ));
    }
    for n in &resolved.notifiers {
        log.info(format!("notifier {}: {} {}", n.name, n.kind, n.url.endpoint()));
    }

    let (notify_tx, notify_rx) = mpsc::channel::<notify::Notification>(256);
    let delivery = tokio::spawn(notify::run_delivery(
        resolved.notifiers.clone(),
        notify_rx,
        log.clone(),
    ));

    let (ev_tx, mut ev_rx) = mpsc::channel::<TargetEvent>(1024);
    let mut statuses: Vec<Arc<Mutex<TargetStatus>>> = Vec::new();
    let mut tasks = JoinSet::new();
    for (i, t) in resolved.targets.iter().cloned().enumerate() {
        let status = Arc::new(Mutex::new(TargetStatus::default()));
        statuses.push(status.clone());
        tasks.spawn(supervisor::run_target(
            i,
            t,
            ev_tx.clone(),
            status,
            log.clone(),
            cancel.clone(),
        ));
    }
    drop(ev_tx);

    if resolved.notify_on_start {
        let _ = notify_tx
            .send(notify::startup_notification(
                &resolved.hostname,
                resolved.targets.len(),
            ))
            .await;
    }

    let mut summary = resolved.summary_every.map(|d| {
        let mut i = tokio::time::interval(d);
        i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        i
    });
    if let Some(s) = summary.as_mut() {
        s.tick().await; // the first tick completes immediately
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            ev = ev_rx.recv() => match ev {
                Some(ev) => {
                    if let Some(n) = notify::render(&ev, &resolved.hostname)
                        && notify_tx.try_send(n).is_err()
                    {
                        log.error(format!("notification queue full; dropped alert for {}", ev.name));
                    }
                }
                None => break,
            },
            _ = async { summary.as_mut().unwrap().tick().await }, if summary.is_some() => {
                for (t, st) in resolved.targets.iter().zip(&statuses) {
                    log.info(summary_line(&t.name, &st.lock().map(|s| s.clone()).unwrap_or_default()));
                }
            }
        }
    }

    log.info("monitor stopping");
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    drop(notify_tx);
    let _ = tokio::time::timeout(Duration::from_secs(15), delivery).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    0
}

fn summary_line(name: &str, st: &TargetStatus) -> String {
    let state = match st.state {
        State::Unknown => "UNKNOWN".to_string(),
        State::Up => format!(
            "UP for {}",
            st.since.map(|s| fmt_duration(s.elapsed())).unwrap_or_default()
        ),
        State::Down => format!(
            "DOWN for {}",
            st.since.map(|s| fmt_duration(s.elapsed())).unwrap_or_default()
        ),
    };
    match &st.last_snapshot {
        // `lost` already includes `failed` (see the loss semantics note in AGENTS.md).
        Some(l) if l.sent > 0 => format!(
            "summary {name}: {state}  sent {} lost {} ({:.2}%)  p95 {:.1} ms  last {}",
            l.sent,
            l.lost,
            l.loss_pct,
            l.p95_ms,
            l.last_ms
                .map(|v| format!("{v:.1} ms"))
                .unwrap_or_else(|| "-".into())
        ),
        _ => format!("summary {name}: {state}  no samples yet"),
    }
}

/// Resolves on Ctrl-C, and on SIGTERM where that exists (systemd stops services with it).
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(t) => t,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `200ms`, `35s`, `4m12s`, `1h03m`, `2d05h`: short enough for a notification title.
pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if d < Duration::from_secs(1) {
        format!("{}ms", d.as_millis())
    } else if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{:02}h", s / 86_400, (s % 86_400) / 3600)
    }
}

/// Best-effort machine name for alert text.
pub fn local_hostname() -> String {
    if let Some(h) = std::env::var_os("COMPUTERNAME")
        .or_else(|| std::env::var_os("HOSTNAME"))
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return h;
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname")
        && !h.trim().is_empty()
    {
        return h.trim().to_string();
    }
    "unknown-host".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(Duration::from_millis(200)), "200ms");
        assert_eq!(fmt_duration(Duration::from_secs(35)), "35s");
        assert_eq!(fmt_duration(Duration::from_secs(252)), "4m12s");
        assert_eq!(fmt_duration(Duration::from_secs(3780)), "1h03m");
        assert_eq!(fmt_duration(Duration::from_secs(190_800)), "2d05h");
    }
}
