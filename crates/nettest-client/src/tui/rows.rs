//! Row model shared by the settings tabs: each tab is a list of rows generated from state on
//! every draw, and a key identifies where a committed value goes. The Test tab wraps the static
//! `form::FIELDS`; the Alerts and Monitor tabs are dynamic because their lists grow.

use nettest_proto::config::ClientConfig;

use super::draft::MonitorDraft;
use super::form::{self, FIELDS, FieldId, FieldKind};
use crate::monitor::config::{self, NotifyConfig, TargetConfig};
use crate::service::ServiceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertField {
    Enabled,
    OnStart,
    OnFinish,
    DownAfter,
    UpAfter,
    RemindEvery,
    Hostname,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifierField {
    Kind,
    Name,
    Url,
    Token,
    Priority,
    TagsDown,
    TagsUp,
    Insecure,
    Fingerprint,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorField {
    SummaryEvery,
    NotifyOnStart,
    Hostname,
    LogFile,
    JsonlFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetField {
    Name,
    Target,
    Interval,
    Timeout,
    Token,
    Insecure,
    Fingerprint,
    WsPath,
    DownAfter,
    UpAfter,
    RemindEvery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKey {
    Test(FieldId),
    Alert(AlertField),
    Notifier(usize, NotifierField),
    Monitor(MonitorField),
    Target(usize, TargetField),
    /// Path of the monitor.toml the draft is written to.
    DraftPath,
    /// Path the Service tab installs from.
    ServiceFrom,
    None,
}

pub struct Row {
    pub key: RowKey,
    pub label: String,
    pub value: String,
    pub kind: FieldKind,
    pub hint: &'static str,
    pub dim: bool,
}

impl Row {
    fn text(key: RowKey, label: impl Into<String>, value: impl Into<String>, hint: &'static str) -> Self {
        Self {
            key,
            label: label.into(),
            value: value.into(),
            kind: FieldKind::Text,
            hint,
            dim: false,
        }
    }
    fn toggle(key: RowKey, label: impl Into<String>, on: bool, hint: &'static str) -> Self {
        Self {
            key,
            label: label.into(),
            value: on_off(on),
            kind: FieldKind::Toggle,
            hint,
            dim: false,
        }
    }
    fn cycle(key: RowKey, label: impl Into<String>, value: impl Into<String>, hint: &'static str) -> Self {
        Self {
            key,
            label: label.into(),
            value: value.into(),
            kind: FieldKind::Cycle,
            hint,
            dim: false,
        }
    }
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            key: RowKey::None,
            label: label.into(),
            value: String::new(),
            kind: FieldKind::Header,
            hint: "",
            dim: false,
        }
    }
    pub fn info(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: RowKey::None,
            label: label.into(),
            value: value.into(),
            kind: FieldKind::Info,
            hint: "",
            dim: false,
        }
    }
}

pub fn on_off(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

// ---- generators ------------------------------------------------------------------------------

pub fn test_rows(cfg: &ClientConfig) -> Vec<Row> {
    FIELDS
        .iter()
        .map(|d| Row {
            key: RowKey::Test(d.id),
            label: d.label.to_string(),
            value: form::value_of(cfg, d.id),
            kind: d.kind,
            hint: d.hint,
            dim: !form::relevant(cfg, d.id),
        })
        .collect()
}

pub fn alerts_rows(cfg: &ClientConfig, draft: &MonitorDraft) -> Vec<Row> {
    let a = &cfg.alerts;
    let mut v = vec![
        Row::header("Alerts for interactive runs (saved in client.toml)"),
        Row::toggle(
            RowKey::Alert(AlertField::Enabled),
            "Send alerts during runs",
            a.enabled,
            "DOWN / UP webhooks while a test runs here, same rules as the monitor service",
        ),
        Row::toggle(
            RowKey::Alert(AlertField::OnStart),
            "Notify on run start",
            a.on_start,
            "confirms the webhooks work",
        ),
        Row::toggle(
            RowKey::Alert(AlertField::OnFinish),
            "Notify on run finish",
            a.on_finish,
            "summary line: loss, rtt p50/p95, disconnects",
        ),
        Row::text(
            RowKey::Alert(AlertField::DownAfter),
            "DOWN after N failures",
            a.failures_before_down.to_string(),
            "consecutive failed probes / disconnects before the DOWN alert",
        ),
        Row::text(
            RowKey::Alert(AlertField::UpAfter),
            "UP after N successes",
            a.successes_before_up.to_string(),
            "consecutive good probes before the UP alert",
        ),
        Row::text(
            RowKey::Alert(AlertField::RemindEvery),
            "Remind while down",
            a.remind_every.clone(),
            "re-send the DOWN alert at this cadence, e.g. 15m; 0 = never",
        ),
        Row::text(
            RowKey::Alert(AlertField::Hostname),
            "This machine's name",
            a.hostname.clone(),
            "used in alert text; empty = detected",
        ),
    ];
    for r in v.iter_mut().skip(2) {
        r.dim = !a.enabled;
    }
    v.push(Row::header(format!(
        "Webhooks: [[notify]] in {} (shared with the service)",
        draft.path.display()
    )));
    if let Some(e) = &draft.load_error {
        v.push(Row::info("cannot parse", e.clone()));
    } else if draft.cfg.notify.is_empty() {
        v.push(Row::info("", "no webhooks yet: press a to add one"));
    }
    for (i, n) in draft.cfg.notify.iter().enumerate() {
        v.extend(notifier_rows(i, n));
    }
    v
}

pub fn notifier_rows(i: usize, n: &NotifyConfig) -> Vec<Row> {
    let k = |f| RowKey::Notifier(i, f);
    let ntfy = n.kind == config::NotifyKind::Ntfy;
    let label = if n.name.trim().is_empty() {
        format!("{}#{}", n.kind, i + 1)
    } else {
        n.name.trim().to_string()
    };
    let mut v = vec![
        Row::header(format!("#{}  {label}", i + 1)),
        Row::cycle(
            k(NotifierField::Kind),
            "  kind",
            n.kind.to_string(),
            "ntfy / slack / discord / teams (Workflows trigger)",
        ),
        Row::text(k(NotifierField::Name), "  name", n.name.clone(), "label in the log; empty = kind#n"),
        Row::text(
            k(NotifierField::Url),
            "  url",
            n.url.clone(),
            "ntfy: https://ntfy.sh/<topic>; Slack/Discord: webhook URL; Teams: workflow URL (sig= is the secret)",
        ),
        Row::text(k(NotifierField::Token), "  token", n.token.clone(), "ntfy access token (Bearer); empty = none"),
        Row::text(
            k(NotifierField::Priority),
            "  priority",
            n.priority.clone(),
            "ntfy priority of DOWN alerts: min/low/default/high/urgent/max or 1-5",
        ),
        Row::text(k(NotifierField::TagsDown), "  tags (down)", n.tags_down.clone(), "ntfy emoji tags for DOWN alerts"),
        Row::text(k(NotifierField::TagsUp), "  tags (up)", n.tags_up.clone(), "ntfy emoji tags for UP alerts"),
        Row::toggle(
            k(NotifierField::Insecure),
            "  TLS: accept any cert",
            n.insecure,
            "self-hosted endpoint with a self-signed certificate",
        ),
        Row::text(
            k(NotifierField::Fingerprint),
            "  TLS: pinned SHA-256",
            n.fingerprint.clone(),
            "pin the endpoint certificate instead; f fetches it from the url",
        ),
        Row::text(k(NotifierField::Timeout), "  timeout", n.timeout.clone(), "per request, e.g. 10s"),
    ];
    for r in v.iter_mut() {
        if matches!(
            r.key,
            RowKey::Notifier(_, NotifierField::Token | NotifierField::Priority | NotifierField::TagsDown | NotifierField::TagsUp)
        ) {
            r.dim = !ntfy;
        }
    }
    v
}

pub fn builder_rows(draft: &MonitorDraft) -> Vec<Row> {
    let m = &draft.cfg.monitor;
    let mut v = vec![
        Row::text(
            RowKey::DraftPath,
            "monitor.toml path",
            draft.path.display().to_string(),
            "where w writes the file; the Service tab installs from here",
        ),
        Row::header("[monitor]"),
        Row::text(
            RowKey::Monitor(MonitorField::SummaryEvery),
            "  summary_every",
            m.summary_every.clone(),
            "status line per target in the log, e.g. 1h; 0 = off",
        ),
        Row::toggle(
            RowKey::Monitor(MonitorField::NotifyOnStart),
            "  notify_on_start",
            m.notify_on_start,
            "send 'monitor started' when the service starts",
        ),
        Row::text(RowKey::Monitor(MonitorField::Hostname), "  hostname", m.hostname.clone(), "name in alert text; empty = detected"),
        Row::text(
            RowKey::Monitor(MonitorField::LogFile),
            "  log_file",
            m.log_file.clone(),
            "leave empty: the service sets its own log path",
        ),
        Row::text(RowKey::Monitor(MonitorField::JsonlFile), "  jsonl_file", m.jsonl_file.clone(), "JSON Lines log; empty = off"),
    ];
    if let Some(e) = &draft.load_error {
        v.push(Row::header("[[targets]]"));
        v.push(Row::info("cannot parse", e.clone()));
    } else if draft.cfg.targets.is_empty() {
        v.push(Row::header("[[targets]]"));
        v.push(Row::info("", "no targets yet: press a to add one"));
    }
    for (i, t) in draft.cfg.targets.iter().enumerate() {
        v.extend(target_rows(i, t));
    }
    v.push(Row::header("[[notify]]"));
    v.push(Row::info(
        "",
        format!(
            "{} webhook(s); edit them on the Alerts tab",
            draft.cfg.notify.len()
        ),
    ));
    v
}

pub fn target_rows(i: usize, t: &TargetConfig) -> Vec<Row> {
    let k = |f| RowKey::Target(i, f);
    let label = if t.name.trim().is_empty() {
        if t.target.trim().is_empty() {
            "(new target)".to_string()
        } else {
            t.target.trim().to_string()
        }
    } else {
        t.name.trim().to_string()
    };
    let tls = t.target.starts_with("wss://") || t.target.starts_with("https://");
    let mut v = vec![
        Row::header(format!("[[targets]] #{}  {label}", i + 1)),
        Row::text(k(TargetField::Name), "  name", t.name.clone(), "label in alerts and the log; empty = the target"),
        Row::text(
            k(TargetField::Target),
            "  target",
            t.target.clone(),
            "ping://10.0.0.7, wss://host:9101, sip://pbx, https://host/path, connect://host:5060 ...",
        ),
        Row::text(k(TargetField::Interval), "  interval", t.interval.clone(), "probe cadence, e.g. 10s (100ms minimum)"),
        Row::text(k(TargetField::Timeout), "  timeout", t.timeout.clone(), "a probe unanswered this long is a failure"),
        Row::text(k(TargetField::Token), "  token", t.token.clone(), "nettest-server --token (ws/wss/tcp/udp only)"),
        Row::toggle(k(TargetField::Insecure), "  TLS: accept any cert", t.insecure, "wss only"),
        Row::text(
            k(TargetField::Fingerprint),
            "  TLS: pinned SHA-256",
            t.fingerprint.clone(),
            "wss / https: f fetches the server certificate's fingerprint",
        ),
        Row::text(k(TargetField::WsPath), "  ws_path", t.ws_path.clone(), "WebSocket or http(s) request path; empty = from the target"),
        Row::text(
            k(TargetField::DownAfter),
            "  failures_before_down",
            t.failures_before_down.to_string(),
            "consecutive failures before the DOWN alert",
        ),
        Row::text(
            k(TargetField::UpAfter),
            "  successes_before_up",
            t.successes_before_up.to_string(),
            "consecutive successes before the UP alert",
        ),
        Row::text(k(TargetField::RemindEvery), "  remind_every", t.remind_every.clone(), "re-send DOWN at this cadence; 0 = never"),
    ];
    for r in v.iter_mut() {
        if matches!(r.key, RowKey::Target(_, TargetField::Insecure | TargetField::Fingerprint)) {
            r.dim = !tls;
        }
    }
    v
}

pub fn service_rows(st: Option<&ServiceStatus>, from: &str, probing: bool) -> Vec<Row> {
    let mut v = vec![Row::header("Monitor service (nettest-monitor)")];
    match st {
        None if probing => v.push(Row::info("status", "querying...")),
        None => v.push(Row::info("status", "press R to query")),
        Some(s) => {
            let privileges = if s.elevated {
                if cfg!(windows) { "Administrator".to_string() } else { "root".to_string() }
            } else if cfg!(windows) {
                format!("user {} - actions will show the UAC prompt", s.user)
            } else {
                format!("user {} - actions will ask for sudo (the screen switches to the prompt)", s.user)
            };
            v.push(Row::info("privileges", privileges));
            v.push(Row::info(
                "installed",
                if !s.supported {
                    "no (service manager not available on this host)".to_string()
                } else if s.installed {
                    "yes".to_string()
                } else {
                    "no".to_string()
                },
            ));
            v.push(Row::info("state", s.state.clone()));
            v.push(Row::info("binary", s.paths.bin.display().to_string()));
            v.push(Row::info("config", s.paths.config.display().to_string()));
            v.push(Row::info("log", s.paths.log_file.display().to_string()));
            if cfg!(target_os = "linux") {
                v.push(Row::info("unit", s.paths.unit.display().to_string()));
            }
        }
    }
    v.push(Row::header("Install"));
    v.push(Row::text(
        RowKey::ServiceFrom,
        "  install from",
        from,
        "monitor.toml copied to the system location by i (defaults to the Monitor tab path)",
    ));
    if let Some(Some(cfg)) = st.map(|s| s.config.as_ref()) {
        v.push(Row::header("Installed configuration"));
        match cfg {
            Ok(lines) => {
                for l in lines {
                    let (k, val) = l.split_at(l.find("  ").unwrap_or(l.len()));
                    v.push(Row::info(k.trim(), val.trim()));
                }
            }
            Err(e) => v.push(Row::info("error", e.clone())),
        }
    }
    v
}

// ---- commits ---------------------------------------------------------------------------------

/// Parse helpers shared by the dynamic tabs.
fn count(what: &str, t: &str) -> Result<u32, String> {
    let n: u32 = t.trim().parse().map_err(|_| format!("{what} must be a whole number"))?;
    if n == 0 {
        return Err(format!("{what} must be at least 1"));
    }
    Ok(n)
}

fn duration(what: &str, t: &str) -> Result<String, String> {
    config::parse_dur(t).map_err(|e| format!("{what}: {e}"))?;
    Ok(t.trim().to_string())
}

fn fingerprint(t: &str) -> Result<String, String> {
    let t = t.trim();
    if !t.is_empty() {
        nettest_proto::tls::fingerprint::parse(t)?;
    }
    Ok(t.to_string())
}

/// Commit an edited text value into whatever the key points at.
pub fn commit(
    key: RowKey,
    text: &str,
    cfg: &mut ClientConfig,
    draft: &mut MonitorDraft,
    service_from: &mut String,
) -> Result<(), String> {
    let t = text.trim();
    match key {
        RowKey::Test(id) => return form::commit(cfg, id, text),
        RowKey::Alert(f) => {
            let a = &mut cfg.alerts;
            match f {
                AlertField::DownAfter => a.failures_before_down = count("DOWN threshold", t)?,
                AlertField::UpAfter => a.successes_before_up = count("UP threshold", t)?,
                AlertField::RemindEvery => a.remind_every = duration("remind", t)?,
                AlertField::Hostname => a.hostname = t.to_string(),
                _ => {}
            }
        }
        RowKey::Notifier(i, f) => {
            let n = draft.cfg.notify.get_mut(i).ok_or("notifier no longer exists")?;
            match f {
                NotifierField::Name => n.name = t.to_string(),
                NotifierField::Url => {
                    if t.is_empty() {
                        return Err("url cannot be empty".into());
                    }
                    n.url = t.to_string();
                }
                NotifierField::Token => n.token = t.to_string(),
                NotifierField::Priority => n.priority = t.to_string(),
                NotifierField::TagsDown => n.tags_down = t.to_string(),
                NotifierField::TagsUp => n.tags_up = t.to_string(),
                NotifierField::Fingerprint => n.fingerprint = fingerprint(t)?,
                NotifierField::Timeout => n.timeout = duration("timeout", t)?,
                NotifierField::Kind | NotifierField::Insecure => {}
            }
            draft.dirty = true;
        }
        RowKey::Monitor(f) => {
            let m = &mut draft.cfg.monitor;
            match f {
                MonitorField::SummaryEvery => m.summary_every = duration("summary_every", t)?,
                MonitorField::Hostname => m.hostname = t.to_string(),
                MonitorField::LogFile => m.log_file = t.to_string(),
                MonitorField::JsonlFile => m.jsonl_file = t.to_string(),
                MonitorField::NotifyOnStart => {}
            }
            draft.dirty = true;
        }
        RowKey::Target(i, f) => {
            let tg = draft.cfg.targets.get_mut(i).ok_or("target no longer exists")?;
            match f {
                TargetField::Name => tg.name = t.to_string(),
                TargetField::Target => {
                    if t.is_empty() {
                        return Err("target cannot be empty".into());
                    }
                    let mut probe = ClientConfig::default();
                    crate::cli::args::parse_target(t, &mut probe)?;
                    tg.target = t.to_string();
                }
                TargetField::Interval => tg.interval = duration("interval", t)?,
                TargetField::Timeout => tg.timeout = duration("timeout", t)?,
                TargetField::Token => tg.token = t.to_string(),
                TargetField::Fingerprint => tg.fingerprint = fingerprint(t)?,
                TargetField::WsPath => tg.ws_path = t.to_string(),
                TargetField::DownAfter => tg.failures_before_down = count("failures_before_down", t)?,
                TargetField::UpAfter => tg.successes_before_up = count("successes_before_up", t)?,
                TargetField::RemindEvery => tg.remind_every = duration("remind_every", t)?,
                TargetField::Insecure => {}
            }
            draft.dirty = true;
        }
        RowKey::DraftPath => {
            if t.is_empty() {
                return Err("path cannot be empty".into());
            }
            draft.path = t.into();
        }
        RowKey::ServiceFrom => *service_from = t.to_string(),
        RowKey::None => {}
    }
    Ok(())
}

/// Enter / Space on a toggle or cycle row.
pub fn cycle(key: RowKey, cfg: &mut ClientConfig, draft: &mut MonitorDraft) {
    match key {
        RowKey::Test(id) => form::cycle(cfg, id),
        RowKey::Alert(AlertField::Enabled) => cfg.alerts.enabled = !cfg.alerts.enabled,
        RowKey::Alert(AlertField::OnStart) => cfg.alerts.on_start = !cfg.alerts.on_start,
        RowKey::Alert(AlertField::OnFinish) => cfg.alerts.on_finish = !cfg.alerts.on_finish,
        RowKey::Notifier(i, NotifierField::Kind) => {
            if let Some(n) = draft.cfg.notify.get_mut(i) {
                n.kind = n.kind.next();
                draft.dirty = true;
            }
        }
        RowKey::Notifier(i, NotifierField::Insecure) => {
            if let Some(n) = draft.cfg.notify.get_mut(i) {
                n.insecure = !n.insecure;
                draft.dirty = true;
            }
        }
        RowKey::Monitor(MonitorField::NotifyOnStart) => {
            draft.cfg.monitor.notify_on_start = !draft.cfg.monitor.notify_on_start;
            draft.dirty = true;
        }
        RowKey::Target(i, TargetField::Insecure) => {
            if let Some(t) = draft.cfg.targets.get_mut(i) {
                t.insecure = !t.insecure;
                draft.dirty = true;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> MonitorDraft {
        MonitorDraft::load(std::env::temp_dir().join("nettest-rows-test-does-not-exist.toml"))
    }

    #[test]
    fn commits_round_trip_into_the_draft() {
        let mut cfg = ClientConfig::default();
        let mut d = draft();
        let mut from = String::new();
        d.cfg.targets.push(TargetConfig::default());
        d.cfg.notify.push(NotifyConfig::default());
        commit(RowKey::Target(0, TargetField::Target), "wss://host:9101", &mut cfg, &mut d, &mut from).unwrap();
        commit(RowKey::Target(0, TargetField::Interval), "30s", &mut cfg, &mut d, &mut from).unwrap();
        assert!(commit(RowKey::Target(0, TargetField::Interval), "soon", &mut cfg, &mut d, &mut from).is_err());
        assert!(commit(RowKey::Target(0, TargetField::DownAfter), "0", &mut cfg, &mut d, &mut from).is_err());
        assert!(commit(RowKey::Target(0, TargetField::Target), "connect://host", &mut cfg, &mut d, &mut from).is_err());
        commit(RowKey::Notifier(0, NotifierField::Url), "https://ntfy.sh/t", &mut cfg, &mut d, &mut from).unwrap();
        assert!(commit(RowKey::Notifier(0, NotifierField::Fingerprint), "zz", &mut cfg, &mut d, &mut from).is_err());
        cycle(RowKey::Notifier(0, NotifierField::Kind), &mut cfg, &mut d);
        assert_eq!(d.cfg.notify[0].kind, config::NotifyKind::Slack);
        commit(RowKey::Alert(AlertField::DownAfter), "5", &mut cfg, &mut d, &mut from).unwrap();
        assert_eq!(cfg.alerts.failures_before_down, 5);
        cycle(RowKey::Alert(AlertField::Enabled), &mut cfg, &mut d);
        assert!(!cfg.alerts.enabled);
        commit(RowKey::ServiceFrom, " /tmp/m.toml ", &mut cfg, &mut d, &mut from).unwrap();
        assert_eq!(from, "/tmp/m.toml");
        assert!(d.dirty);

        let text = config::to_toml(&d.cfg).unwrap();
        let back = config::parse_str(&text).unwrap();
        assert_eq!(back.targets[0].target, "wss://host:9101");
        assert_eq!(back.targets[0].interval, "30s");
        assert_eq!(back.notify[0].kind, config::NotifyKind::Slack);
        // wss without insecure/fingerprint is the one thing resolve must still reject.
        let errs = config::resolve(&back).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("wss needs")), "{errs:?}");
    }

    #[test]
    fn row_lists_cover_every_entry() {
        let cfg = ClientConfig::default();
        let mut d = draft();
        d.cfg.targets.push(TargetConfig { target: "ping://10.0.0.1".into(), ..Default::default() });
        d.cfg.targets.push(TargetConfig { target: "wss://h:9101".into(), ..Default::default() });
        d.cfg.notify.push(NotifyConfig { url: "https://ntfy.sh/x".into(), ..Default::default() });
        let b = builder_rows(&d);
        assert_eq!(b.iter().filter(|r| r.kind == FieldKind::Header && r.label.starts_with("[[targets]]")).count(), 2);
        assert!(b.iter().any(|r| r.key == RowKey::Target(1, TargetField::Fingerprint) && !r.dim));
        assert!(b.iter().any(|r| r.key == RowKey::Target(0, TargetField::Fingerprint) && r.dim));
        let a = alerts_rows(&cfg, &d);
        assert!(a.iter().any(|r| r.key == RowKey::Notifier(0, NotifierField::Url)));
        assert_eq!(test_rows(&cfg).len(), FIELDS.len());
        let s = service_rows(None, "/x", false);
        assert!(s.iter().any(|r| r.key == RowKey::ServiceFrom));
    }
}
