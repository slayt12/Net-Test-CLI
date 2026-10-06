//! Webhook alerts for an interactive run: the same `Health` state machine and message text the
//! monitor service uses, fed from the runner's `UiEvent`s instead of a supervisor.
//!
//! Delivery is spawned per notifier (`notify::deliver`, with its retries) and reported back to
//! the app over the background channel, so a slow webhook never stalls drawing or probing.

use std::time::Instant;

use nettest_proto::config::ClientConfig;
use nettest_proto::stats::RunSummary;
use nettest_proto::transport::Protocol;

use crate::monitor::config::{self, ResolvedNotifier};
use crate::monitor::health::{Health, Thresholds};
use crate::monitor::notify::{self, Kind, Notification};
use crate::monitor::supervisor::{TargetEvent, signal_of};
use crate::runner::UiEvent;

pub struct RunAlerts {
    pub notifiers: Vec<ResolvedNotifier>,
    health: Health,
    hostname: String,
    target: String,
    protocol: Protocol,
    pub on_finish: bool,
    pub on_start: bool,
}

impl RunAlerts {
    /// `Ok(None)` when alerts are off or no notifier is configured; `Err` when the notifier
    /// list has a problem (the run should not start silently without the alerts the user set up).
    pub fn prepare(
        cfg: &ClientConfig,
        notify: &[config::NotifyConfig],
    ) -> Result<Option<(Self, Vec<String>)>, Vec<String>> {
        if !cfg.alerts.enabled || notify.is_empty() {
            return Ok(None);
        }
        let (notifiers, warnings) = config::resolve_notifiers(notify)?;
        let remind = match config::parse_dur(&cfg.alerts.remind_every) {
            Ok(d) => (!d.is_zero()).then_some(d),
            Err(e) => return Err(vec![format!("alerts remind_every: {e}")]),
        };
        let hostname = if cfg.alerts.hostname.trim().is_empty() {
            crate::monitor::local_hostname()
        } else {
            cfg.alerts.hostname.trim().to_string()
        };
        let a = Self {
            notifiers,
            health: Health::new(Thresholds {
                down_after: cfg.alerts.failures_before_down.max(1),
                up_after: cfg.alerts.successes_before_up.max(1),
                remind_every: remind,
            }),
            hostname: format!("{hostname} (interactive run)"),
            target: cfg.target(),
            protocol: cfg.protocol,
            on_finish: cfg.alerts.on_finish,
            on_start: cfg.alerts.on_start,
        };
        Ok(Some((a, warnings)))
    }

    fn event(&self, transition: crate::monitor::health::Transition) -> TargetEvent {
        TargetEvent {
            index: 0,
            name: self.target.clone(),
            target: self.target.clone(),
            at: chrono::Local::now(),
            transition,
        }
    }

    pub fn on_event(&mut self, ev: &UiEvent) -> Option<Notification> {
        let signal = signal_of(self.protocol, ev)?;
        let t = self.health.on_signal(signal, Instant::now())?;
        notify::render(&self.event(t), &self.hostname)
    }

    pub fn on_tick(&mut self) -> Option<Notification> {
        let t = self.health.on_tick(Instant::now())?;
        notify::render(&self.event(t), &self.hostname)
    }

    pub fn started(&self, cfg: &ClientConfig) -> Notification {
        Notification {
            kind: Kind::Info,
            title: format!("nettest: {} test started", cfg.mode),
            body: format!(
                "{} test of {} started on {}",
                cfg.mode, self.target, self.hostname
            ),
        }
    }

    pub fn finished(&self, s: &RunSummary) -> Notification {
        let l = &s.latency;
        let mut body = format!(
            "{} test of {} finished ({}) on {}",
            s.mode, self.target, s.stop_reason, self.hostname
        );
        if l.sent > 0 {
            body.push_str(&format!(
                ": sent {} received {} lost {} ({:.2}%), rtt p50 {:.2} / p95 {:.2} ms",
                l.sent, l.received, l.lost, l.loss_pct, l.p50_ms, l.p95_ms
            ));
        }
        if let Some(k) = &s.soak {
            body.push_str(&format!(
                ", {} disconnect(s), uptime {:.3}%, longest outage {} ms",
                k.disconnects, k.uptime_pct, k.longest_outage_ms
            ));
        }
        if let Some(t) = &s.throughput {
            body.push_str(&format!(
                ", up {:.2} / down {:.2} Mbit/s",
                t.up_mbps, t.down_mbps
            ));
        }
        Notification {
            kind: Kind::Info,
            title: format!("nettest: {} test finished", s.mode),
            body,
        }
    }
}
