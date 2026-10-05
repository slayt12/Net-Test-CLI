//! Alert delivery: turn a target transition into a message and POST it to every configured
//! notifier (ntfy, Slack incoming webhook, Discord webhook) with retries.
//!
//! One delivery task drains a bounded queue so a slow endpoint never stalls monitoring. Retry
//! policy: 3 attempts; transport errors and 5xx retry after 2 s then 5 s; 429 honours
//! `Retry-After` (capped at 30 s); any other 4xx is permanent (a bad URL will not fix itself).
//!
//! Wire formats:
//! - ntfy: `POST <topic url>`, text body, `Title` / `Priority` / `Tags` headers, optional
//!   `Authorization: Bearer <token>`. Headers must be ASCII, so the title is sanitised.
//! - Slack: `POST <webhook url>` with JSON `{"text": ...}`; replies `200 ok`.
//! - Discord: `POST <webhook url>` with JSON `{"content": ..., "username": "nettest"}`;
//!   replies `204 No Content`; `content` is limited to 2000 characters.

use std::time::Duration;

use futures_util::future::join_all;
use nettest_proto::http::{HttpError, HttpOptions, Response, post};
use nettest_proto::log::LineLog;
use tokio::sync::mpsc;

use super::config::{NotifyKind, ResolvedNotifier};
use super::fmt_duration;
use super::health::Transition;
use super::supervisor::TargetEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Down,
    Up,
    Remind,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

/// Message for a transition; `None` for a healthy first verdict (nothing to announce).
pub fn render(ev: &TargetEvent, hostname: &str) -> Option<Notification> {
    let when = ev.at.format("%Y-%m-%d %H:%M:%S %Z");
    let (kind, title, body) = match &ev.transition {
        Transition::Initial { up: true, .. } => return None,
        Transition::Initial { up: false, reason } => (
            Kind::Down,
            format!("nettest: {} DOWN", ev.name),
            format!(
                "{} ({}) is DOWN since {} (already down when the monitor started): {} — monitor on {}",
                ev.name, ev.target, when, reason, hostname
            ),
        ),
        Transition::Down { reason, consecutive } => (
            Kind::Down,
            format!("nettest: {} DOWN", ev.name),
            format!(
                "{} ({}) is DOWN since {} after {} consecutive failures: {} — monitor on {}",
                ev.name, ev.target, when, consecutive, reason, hostname
            ),
        ),
        Transition::Up { downtime, .. } => (
            Kind::Up,
            format!("nettest: {} UP", ev.name),
            format!(
                "{} ({}) is UP again at {} after {} down — monitor on {}",
                ev.name,
                ev.target,
                when,
                fmt_duration(*downtime),
                hostname
            ),
        ),
        Transition::Remind { downtime, reason } => (
            Kind::Remind,
            format!("nettest: {} still DOWN", ev.name),
            format!(
                "{} ({}) has been DOWN for {}: {} — monitor on {}",
                ev.name,
                ev.target,
                fmt_duration(*downtime),
                reason,
                hostname
            ),
        ),
    };
    Some(Notification { kind, title, body })
}

pub fn startup_notification(hostname: &str, targets: usize) -> Notification {
    Notification {
        kind: Kind::Info,
        title: "nettest: monitor started".into(),
        body: format!(
            "nettest-client {} monitor started on {} watching {} target(s)",
            env!("CARGO_PKG_VERSION"),
            hostname,
            targets
        ),
    }
}

pub fn test_notification(hostname: &str, targets: usize) -> Notification {
    Notification {
        kind: Kind::Info,
        title: "nettest: test notification".into(),
        body: format!(
            "This is a test from nettest-client {} on {} ({} target(s) configured). If you can read this, the webhook works.",
            env!("CARGO_PKG_VERSION"),
            hostname,
            targets
        ),
    }
}

/// Single attempt. `Ok(status)` on 2xx.
pub async fn send_one(n: &ResolvedNotifier, msg: &Notification) -> Result<Response, Attempt> {
    let opts = HttpOptions {
        timeout: n.timeout,
        tls: n.tls,
        ..Default::default()
    };
    let auth = format!("Bearer {}", n.token);
    let res = match n.kind {
        NotifyKind::Ntfy => {
            let title = ascii_only(&msg.title);
            let priority = match msg.kind {
                Kind::Down | Kind::Remind => n.priority.as_str(),
                Kind::Up | Kind::Info => "default",
            };
            let tags = match msg.kind {
                Kind::Down | Kind::Remind => n.tags_down.as_str(),
                Kind::Up => n.tags_up.as_str(),
                Kind::Info => "",
            };
            let mut headers: Vec<(&str, &str)> = vec![("Title", &title), ("Priority", priority)];
            if !tags.is_empty() {
                headers.push(("Tags", tags));
            }
            if !n.token.is_empty() {
                headers.push(("Authorization", &auth));
            }
            post(
                &n.url,
                &headers,
                "text/plain; charset=utf-8",
                msg.body.as_bytes(),
                &opts,
            )
            .await
        }
        NotifyKind::Slack => {
            let body = serde_json::json!({ "text": format!("*{}*\n{}", msg.title, msg.body) });
            post(&n.url, &[], "application/json", body.to_string().as_bytes(), &opts).await
        }
        NotifyKind::Discord => {
            let content = truncate_chars(&format!("**{}**\n{}", msg.title, msg.body), 2000);
            let body = serde_json::json!({ "content": content, "username": "nettest" });
            post(&n.url, &[], "application/json", body.to_string().as_bytes(), &opts).await
        }
    };
    match res {
        Ok(r) if r.is_success() => Ok(r),
        Ok(r) => Err(Attempt::Http(r)),
        Err(e) => Err(Attempt::Transport(e)),
    }
}

#[derive(Debug)]
pub enum Attempt {
    Http(Response),
    Transport(HttpError),
}

impl Attempt {
    fn retryable(&self) -> bool {
        match self {
            Attempt::Transport(_) => true,
            Attempt::Http(r) => r.status >= 500 || r.status == 429 || r.status == 408,
        }
    }

    fn describe(&self) -> String {
        match self {
            Attempt::Transport(e) => e.to_string(),
            Attempt::Http(r) => {
                let body = r.body_text();
                let body = body.trim();
                if body.is_empty() {
                    format!("HTTP {} {}", r.status, r.reason)
                } else {
                    format!("HTTP {} {}: {:.120}", r.status, r.reason, body)
                }
            }
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Attempt::Http(r) if r.status == 429 => r
                .header("retry-after")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|s| Duration::from_secs_f64(s.clamp(1.0, 30.0))),
            _ => None,
        }
    }
}

/// Up to three attempts. Returns `(status, attempts)` or the last failure text.
pub async fn deliver(n: &ResolvedNotifier, msg: &Notification) -> Result<(u16, u32), String> {
    let waits = [Duration::from_secs(2), Duration::from_secs(5)];
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match send_one(n, msg).await {
            Ok(r) => return Ok((r.status, attempt)),
            Err(e) => {
                if !e.retryable() || attempt > waits.len() as u32 {
                    return Err(format!("{} (after {attempt} attempt(s))", e.describe()));
                }
                let wait = e.retry_after().unwrap_or(waits[attempt as usize - 1]);
                tokio::time::sleep(wait).await;
            }
        }
    }
}

/// Deliver to every notifier concurrently, logging each outcome. Returns how many failed.
pub async fn deliver_all(notifiers: &[ResolvedNotifier], msg: &Notification, log: &LineLog) -> usize {
    let t0 = std::time::Instant::now();
    let results = join_all(notifiers.iter().map(|n| deliver(n, msg))).await;
    let mut failed = 0;
    for (n, r) in notifiers.iter().zip(results) {
        match r {
            Ok((status, attempts)) => log.info(format!(
                "notify {} ({}): delivered \"{}\" ({status}{}) in {} ms",
                n.name,
                n.kind,
                msg.title,
                if attempts > 1 {
                    format!(", attempt {attempts}")
                } else {
                    String::new()
                },
                t0.elapsed().as_millis()
            )),
            Err(e) => {
                failed += 1;
                log.error(format!(
                    "notify {} ({}): FAILED \"{}\": {e}",
                    n.name, n.kind, msg.title
                ));
            }
        }
    }
    failed
}

/// The single delivery task.
pub async fn run_delivery(
    notifiers: Vec<ResolvedNotifier>,
    mut rx: mpsc::Receiver<Notification>,
    log: LineLog,
) {
    while let Some(msg) = rx.recv().await {
        if notifiers.is_empty() {
            continue;
        }
        deliver_all(&notifiers, &msg, &log).await;
    }
}

fn ascii_only(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' })
        .collect()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tr: Transition) -> TargetEvent {
        TargetEvent {
            index: 0,
            name: "pbx".into(),
            target: "sip://10.0.0.5:5060".into(),
            at: chrono::Local::now(),
            transition: tr,
        }
    }

    #[test]
    fn rendering() {
        assert!(render(&ev(Transition::Initial { up: true, reason: String::new() }), "h").is_none());
        let d = render(
            &ev(Transition::Down { reason: "timed out".into(), consecutive: 3 }),
            "office-pc",
        )
        .unwrap();
        assert_eq!(d.kind, Kind::Down);
        assert_eq!(d.title, "nettest: pbx DOWN");
        assert!(d.body.contains("after 3 consecutive failures: timed out"));
        assert!(d.body.contains("monitor on office-pc"));
        let u = render(&ev(Transition::Up { downtime: Duration::from_secs(252), successes: 1 }), "h").unwrap();
        assert_eq!((u.kind, u.title.as_str()), (Kind::Up, "nettest: pbx UP"));
        assert!(u.body.contains("after 4m12s down"));
        let r = render(&ev(Transition::Remind { downtime: Duration::from_secs(3600), reason: "x".into() }), "h").unwrap();
        assert_eq!(r.kind, Kind::Remind);
        assert!(r.body.contains("DOWN for 1h00m"));
        assert_eq!(ascii_only("héllo\n"), "h?llo?");
        assert_eq!(truncate_chars("abcdef", 4).chars().count(), 4);
        assert_eq!(truncate_chars("abc", 4), "abc");
    }

    #[test]
    fn retry_policy() {
        let r = |status| Attempt::Http(Response { status, reason: String::new(), headers: vec![("Retry-After".into(), "7".into())], body: vec![] });
        assert!(r(500).retryable());
        assert!(r(429).retryable());
        assert!(!r(400).retryable());
        assert!(!r(404).retryable());
        assert_eq!(r(429).retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(r(503).retry_after(), None);
        assert!(Attempt::Transport(HttpError::Timeout).retryable());
    }
}
