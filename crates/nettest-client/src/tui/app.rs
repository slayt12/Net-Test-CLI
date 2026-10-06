//! Application state and key handling. Rendering is delegated to `views`.
//!
//! The settings screen is tabbed: Test (the run settings), Alerts (webhooks + alert rules),
//! Monitor (a `monitor.toml` builder) and Service (install / control the monitor service).
//! Slow work (certificate fetch, webhook delivery, service commands, status probes) runs in
//! spawned tasks and reports back through `bg_rx`, so drawing never blocks. A privileged
//! service command on Linux needs the terminal for sudo's prompt: the app sets `suspend` and the
//! event loop hands the terminal over and back.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::sinks::{Event, LogLevel, MultiSink};
use nettest_proto::stats::{ChartPoint, RunSummary};
use ratatui::Frame;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::alerts::RunAlerts;
use super::draft::MonitorDraft;
use super::edit::{EditOutcome, Editor};
use super::form::{self, FieldId, FieldKind};
use super::rows::{self, NotifierField, Row, RowKey, TargetField};
use crate::monitor::config::{self, NotifyConfig, ResolvedNotifier, TargetConfig};
use crate::monitor::notify::{self, Notification};
use crate::output::{OutputPaths, build_file_sinks, write_report};
use crate::runner::{Command, RunOutput, Runner, Snapshot, UiEvent};
use crate::service::{self, ServiceStatus, Verb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Form,
    Running,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Test,
    Alerts,
    Monitor,
    Service,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Test, Tab::Alerts, Tab::Monitor, Tab::Service];
    pub fn title(self) -> &'static str {
        match self {
            Tab::Test => "Test",
            Tab::Alerts => "Alerts",
            Tab::Monitor => "Monitor",
            Tab::Service => "Service",
        }
    }
    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
    fn next(self) -> Self {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }
    fn prev(self) -> Self {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartWindow {
    Last60,
    Last300,
    All,
}

impl ChartWindow {
    pub fn secs(self) -> Option<f64> {
        match self {
            ChartWindow::Last60 => Some(60.0),
            ChartWindow::Last300 => Some(300.0),
            ChartWindow::All => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ChartWindow::Last60 => "60s",
            ChartWindow::Last300 => "5m",
            ChartWindow::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Connected,
    Reconnecting { attempt: u32 },
    Idle,
}

pub struct LogLine {
    pub level: LogLevel,
    pub at: chrono::DateTime<chrono::Local>,
    pub text: String,
}

/// A pending `y/n` question shown in the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    DeleteTarget(usize),
    DeleteNotifier(usize),
    Reload,
    Service(Verb),
}

/// Which result pane a background outcome belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Run,
    Alerts,
    Monitor,
}

/// Outcomes of spawned work.
pub enum BgEvent {
    Fingerprint {
        key: RowKey,
        result: Result<(nettest_proto::tls::fingerprint::Fingerprint, std::net::SocketAddr), String>,
    },
    Notify {
        pane: Pane,
        name: String,
        title: String,
        result: Result<(u16, u32), String>,
    },
    Status(ServiceStatus),
    ServiceDone {
        verb: Verb,
        result: Result<(i32, String), String>,
    },
}

pub struct ServiceTab {
    pub status: Option<ServiceStatus>,
    pub probing: bool,
    pub busy: Option<Verb>,
    pub from: String,
    pub output: Vec<(LogLevel, String)>,
}

pub struct App {
    pub cfg: ClientConfig,
    pub config_path: PathBuf,
    pub screen: Screen,
    pub tab: Tab,
    pub should_quit: bool,
    pub show_help: bool,

    // Settings tabs
    pub field: usize,
    pub alerts_cursor: usize,
    pub builder_cursor: usize,
    pub service_cursor: usize,
    pub editing: Option<Editor>,
    pub confirm: Option<Confirm>,
    pub status: Option<(LogLevel, String)>,
    pub draft: MonitorDraft,
    pub alerts_results: Vec<(LogLevel, String)>,
    pub builder_results: Vec<(LogLevel, String)>,
    pub service: ServiceTab,
    /// Privileged command the event loop must run with the terminal restored (Linux sudo).
    pub suspend: Option<Vec<OsString>>,
    bg_tx: mpsc::Sender<BgEvent>,
    pub bg_rx: mpsc::Receiver<BgEvent>,
    notes_shown: bool,

    // Run
    pub runner_rx: Option<mpsc::Receiver<UiEvent>>,
    cmd_tx: Option<mpsc::Sender<Command>>,
    join: Option<JoinHandle<RunOutput>>,
    run_alerts: Option<RunAlerts>,
    alerts_in_flight: usize,
    pub snapshot: Snapshot,
    pub link: Link,
    pub log: VecDeque<LogLine>,
    pub events: Vec<Event>,
    pub tp_series: Vec<(f64, f64)>,
    pub window: ChartWindow,
    pub run_cfg: ClientConfig,
    paths: Option<OutputPaths>,
    pub last_output: Option<RunOutput>,
    pub report_path: Option<PathBuf>,
    pub stop_requested: bool,
}

const LOG_CAP: usize = 300;
const RESULTS_CAP: usize = 60;

impl App {
    pub fn new(cfg: ClientConfig, config_path: PathBuf, monitor_path: PathBuf) -> Self {
        let (bg_tx, bg_rx) = mpsc::channel(64);
        let draft = MonitorDraft::load(monitor_path);
        let from = draft.path.display().to_string();
        Self {
            run_cfg: cfg.clone(),
            cfg,
            config_path,
            screen: Screen::Form,
            tab: Tab::Test,
            should_quit: false,
            show_help: false,
            field: 0,
            alerts_cursor: 0,
            builder_cursor: 0,
            service_cursor: 0,
            editing: None,
            confirm: None,
            status: None,
            draft,
            alerts_results: Vec::new(),
            builder_results: Vec::new(),
            service: ServiceTab {
                status: None,
                probing: false,
                busy: None,
                from,
                output: Vec::new(),
            },
            suspend: None,
            bg_tx,
            bg_rx,
            notes_shown: false,
            runner_rx: None,
            cmd_tx: None,
            join: None,
            run_alerts: None,
            alerts_in_flight: 0,
            snapshot: Snapshot::default(),
            link: Link::Idle,
            log: VecDeque::with_capacity(LOG_CAP),
            events: Vec::new(),
            tp_series: Vec::new(),
            window: ChartWindow::Last60,
            paths: None,
            last_output: None,
            report_path: None,
            stop_requested: false,
        }
    }

    pub fn render(&mut self, f: &mut Frame) {
        super::views::render(self, f);
    }

    pub fn is_running(&self) -> bool {
        self.join.is_some()
    }

    fn set_status(&mut self, level: LogLevel, text: impl Into<String>) {
        self.status = Some((level, text.into()));
    }

    fn push_log(&mut self, level: LogLevel, at: chrono::DateTime<chrono::Local>, text: String) {
        if self.log.len() == LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogLine { level, at, text });
    }

    fn push_result(&mut self, pane: Pane, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        let v = match pane {
            Pane::Run => {
                self.push_log(level, chrono::Local::now(), text);
                return;
            }
            Pane::Alerts => &mut self.alerts_results,
            Pane::Monitor => &mut self.builder_results,
        };
        if v.len() == RESULTS_CAP {
            v.remove(0);
        }
        v.push((level, text));
    }

    // ----- rows / cursor ----------------------------------------------------------------

    pub fn rows(&self) -> Vec<Row> {
        match self.tab {
            Tab::Test => rows::test_rows(&self.cfg),
            Tab::Alerts => rows::alerts_rows(&self.cfg, &self.draft),
            Tab::Monitor => rows::builder_rows(&self.draft),
            Tab::Service => rows::service_rows(
                self.service.status.as_ref(),
                &self.service.from,
                self.service.probing,
            ),
        }
    }

    pub fn cursor(&self) -> usize {
        match self.tab {
            Tab::Test => self.field,
            Tab::Alerts => self.alerts_cursor,
            Tab::Monitor => self.builder_cursor,
            Tab::Service => self.service_cursor,
        }
    }

    fn cursor_mut(&mut self) -> &mut usize {
        match self.tab {
            Tab::Test => &mut self.field,
            Tab::Alerts => &mut self.alerts_cursor,
            Tab::Monitor => &mut self.builder_cursor,
            Tab::Service => &mut self.service_cursor,
        }
    }

    /// Move the cursor by `delta` rows, skipping headers / read-only lines, wrapping around.
    fn move_cursor(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let n = rows.len() as isize;
        let mut i = self.cursor() as isize;
        for _ in 0..rows.len() {
            i = (i + delta).rem_euclid(n);
            if rows[i as usize].kind.selectable() {
                *self.cursor_mut() = i as usize;
                return;
            }
        }
    }

    fn cursor_to_edge(&mut self, last: bool) {
        let rows = self.rows();
        let pick = if last {
            rows.iter().rposition(|r| r.kind.selectable())
        } else {
            rows.iter().position(|r| r.kind.selectable())
        };
        if let Some(i) = pick {
            *self.cursor_mut() = i;
        }
    }

    /// Keep the cursor on a selectable row after rows were added or removed.
    fn clamp_cursor(&mut self) {
        let rows = self.rows();
        let c = self.cursor().min(rows.len().saturating_sub(1));
        *self.cursor_mut() = c;
        if rows.get(c).is_some_and(|r| !r.kind.selectable()) {
            self.move_cursor(-1);
        }
    }

    pub fn current_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.cursor())
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.editing = None;
        self.confirm = None;
        self.status = None;
        self.clamp_cursor();
        if tab == Tab::Service && self.service.status.is_none() && !self.service.probing {
            self.probe_service();
        }
        if !self.notes_shown && matches!(tab, Tab::Alerts | Tab::Monitor) {
            self.notes_shown = true;
            if let Some(e) = &self.draft.load_error {
                self.set_status(
                    LogLevel::Error,
                    format!(
                        "{} could not be parsed: {e} (fix it and press l, or D to start over)",
                        self.draft.path.display()
                    ),
                );
            } else if self.draft.exists() {
                self.set_status(
                    LogLevel::Info,
                    format!(
                        "loaded {}{}; w rewrites it in full (comments are not kept)",
                        self.draft.path.display(),
                        self.draft
                            .encoding
                            .filter(|e| !e.is_utf8())
                            .map(|e| format!(" ({e}, saved as UTF-8)"))
                            .unwrap_or_default()
                    ),
                );
            }
        }
    }

    // ----- keys -------------------------------------------------------------------------

    pub async fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.request_quit().await;
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        match self.screen {
            Screen::Form => self.on_key_form(k).await,
            Screen::Running => self.on_key_running(k).await,
            Screen::Summary => self.on_key_summary(k).await,
        }
    }

    async fn request_quit(&mut self) {
        if self.is_running() {
            self.stop_requested = true;
            self.should_quit = true;
            if let Some(tx) = &self.cmd_tx {
                let _ = tx.send(Command::Stop).await;
            }
        } else {
            self.should_quit = true;
        }
    }

    async fn on_key_form(&mut self, k: KeyEvent) {
        if let Some(ed) = self.editing.as_mut() {
            match ed.on_key(k) {
                EditOutcome::Editing => {}
                EditOutcome::Cancel => self.editing = None,
                EditOutcome::Commit => {
                    let text = ed.text();
                    let key = self.current_row().map(|r| r.key).unwrap_or(RowKey::None);
                    match rows::commit(
                        key,
                        &text,
                        &mut self.cfg,
                        &mut self.draft,
                        &mut self.service.from,
                    ) {
                        Ok(()) => {
                            self.editing = None;
                            self.status = None;
                        }
                        Err(e) => self.set_status(LogLevel::Error, e),
                    }
                }
            }
            return;
        }
        if let Some(c) = self.confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                self.execute_confirmed(c);
            } else {
                self.set_status(LogLevel::Info, "cancelled");
            }
            return;
        }
        // Keys common to every tab.
        match k.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                return;
            }
            KeyCode::Right | KeyCode::PageDown => {
                self.switch_tab(self.tab.next());
                return;
            }
            KeyCode::Left | KeyCode::PageUp => {
                self.switch_tab(self.tab.prev());
                return;
            }
            KeyCode::F(n @ 1..=4) => {
                self.switch_tab(Tab::ALL[n as usize - 1]);
                return;
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.move_cursor(1);
                return;
            }
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.move_cursor(-1);
                return;
            }
            KeyCode::Home => {
                self.cursor_to_edge(false);
                return;
            }
            KeyCode::End => {
                self.cursor_to_edge(true);
                return;
            }
            _ => {}
        }
        let row = self.current_row();
        if let Some(row) = &row {
            match k.code {
                KeyCode::Enter | KeyCode::Char(' ')
                    if matches!(row.kind, FieldKind::Toggle | FieldKind::Cycle) =>
                {
                    rows::cycle(row.key, &mut self.cfg, &mut self.draft);
                    return;
                }
                KeyCode::Enter if row.kind == FieldKind::Text => {
                    self.editing = Some(Editor::new(&row.value));
                    return;
                }
                _ => {}
            }
        }
        match self.tab {
            Tab::Test => self.on_key_test(k).await,
            Tab::Alerts => self.on_key_alerts(k),
            Tab::Monitor => self.on_key_builder(k),
            Tab::Service => self.on_key_service(k),
        }
    }

    async fn on_key_test(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('w') => self.save_client_config(),
            KeyCode::Char('s') | KeyCode::F(5) => self.start().await,
            KeyCode::Char('f') => {
                if self.cfg.protocol.is_tls() || self.cfg.protocol == nettest_proto::transport::Protocol::Https {
                    let host = self.cfg.host.clone();
                    let port = self.cfg.port;
                    let ipv6 = self.cfg.ipv6;
                    self.fetch_fingerprint(RowKey::Test(FieldId::Fingerprint), host, port, ipv6);
                } else {
                    self.set_status(
                        LogLevel::Warn,
                        format!("{} has no TLS certificate to fetch (wss / https only)", self.cfg.protocol),
                    );
                }
            }
            _ => {}
        }
    }

    fn save_client_config(&mut self) {
        match nettest_proto::config::save(&self.config_path, &self.cfg) {
            Ok(()) => self.set_status(
                LogLevel::Info,
                format!("settings saved to {}", self.config_path.display()),
            ),
            Err(e) => self.set_status(LogLevel::Error, format!("save failed: {e}")),
        }
    }

    fn save_draft(&mut self) -> bool {
        match self.draft.save() {
            Ok(()) => {
                self.set_status(
                    LogLevel::Info,
                    format!("monitor.toml written to {}", self.draft.path.display()),
                );
                true
            }
            Err(e) => {
                self.set_status(LogLevel::Error, e);
                false
            }
        }
    }

    fn on_key_alerts(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('a') => {
                if self.draft.load_error.is_some() {
                    self.set_status(LogLevel::Error, "fix or discard (D) the unparsable monitor.toml first");
                    return;
                }
                self.draft.cfg.notify.push(NotifyConfig::default());
                self.draft.dirty = true;
                let i = self.draft.cfg.notify.len() - 1;
                self.jump_to(RowKey::Notifier(i, NotifierField::Url));
                self.set_status(LogLevel::Info, "added a notifier: set its kind and url");
            }
            KeyCode::Char('d') => {
                if let Some(Row { key: RowKey::Notifier(i, _), .. }) = self.current_row() {
                    self.confirm = Some(Confirm::DeleteNotifier(i));
                } else {
                    self.set_status(LogLevel::Warn, "move onto a notifier to delete it");
                }
            }
            KeyCode::Char('f') => self.fetch_for_current_row(),
            KeyCode::Char('t') => self.test_notify(Pane::Alerts),
            KeyCode::Char('w') => {
                if self.save_draft() {
                    match nettest_proto::config::save(&self.config_path, &self.cfg) {
                        Ok(()) => self.set_status(
                            LogLevel::Info,
                            format!(
                                "saved {} and {}",
                                self.draft.path.display(),
                                self.config_path.display()
                            ),
                        ),
                        Err(e) => self.set_status(LogLevel::Error, format!("client.toml save failed: {e}")),
                    }
                }
            }
            KeyCode::Char('D') if self.draft.load_error.is_some() => {
                self.draft.discard_load_error();
                self.set_status(LogLevel::Warn, "starting from an empty monitor.toml; w overwrites the old file");
            }
            KeyCode::Char('c') => self.alerts_results.clear(),
            _ => {}
        }
    }

    fn on_key_builder(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('a') => {
                if self.draft.load_error.is_some() {
                    self.set_status(LogLevel::Error, "fix or discard (D) the unparsable monitor.toml first");
                    return;
                }
                // Seed from the Test tab so the usual case (monitor what you just tested) is
                // one keystroke.
                let t = TargetConfig {
                    target: self.cfg.target(),
                    token: self.cfg.token.clone(),
                    insecure: self.cfg.insecure,
                    fingerprint: self.cfg.fingerprint.clone(),
                    ..TargetConfig::default()
                };
                self.draft.cfg.targets.push(t);
                self.draft.dirty = true;
                let i = self.draft.cfg.targets.len() - 1;
                self.jump_to(RowKey::Target(i, TargetField::Target));
                self.set_status(LogLevel::Info, "added a target (seeded from the Test tab)");
            }
            KeyCode::Char('d') => {
                if let Some(Row { key: RowKey::Target(i, _), .. }) = self.current_row() {
                    self.confirm = Some(Confirm::DeleteTarget(i));
                } else {
                    self.set_status(LogLevel::Warn, "move onto a target to delete it");
                }
            }
            KeyCode::Char('f') => self.fetch_for_current_row(),
            KeyCode::Char('v') => self.verify_draft(),
            KeyCode::Char('t') => self.test_notify(Pane::Monitor),
            KeyCode::Char('w') => {
                if self.save_draft() {
                    self.service.from = self.draft.path.display().to_string();
                }
            }
            KeyCode::Char('l') => {
                if self.draft.dirty {
                    self.confirm = Some(Confirm::Reload);
                } else {
                    self.draft.reload();
                    self.clamp_cursor();
                    self.set_status(LogLevel::Info, format!("reloaded {}", self.draft.path.display()));
                }
            }
            KeyCode::Char('D') if self.draft.load_error.is_some() => {
                self.draft.discard_load_error();
                self.set_status(LogLevel::Warn, "starting from an empty monitor.toml; w overwrites the old file");
            }
            KeyCode::Char('c') => self.builder_results.clear(),
            _ => {}
        }
    }

    fn on_key_service(&mut self, k: KeyEvent) {
        let verb = match k.code {
            KeyCode::Char('i') => Some(Verb::Install),
            KeyCode::Char('s') => Some(Verb::Start),
            KeyCode::Char('x') => Some(Verb::Stop),
            KeyCode::Char('r') => Some(Verb::Restart),
            KeyCode::Char('u') => Some(Verb::Uninstall),
            KeyCode::Char('U') => Some(Verb::Purge),
            KeyCode::Char('R') => {
                self.probe_service();
                None
            }
            KeyCode::Char('c') => {
                self.service.output.clear();
                None
            }
            _ => None,
        };
        let Some(verb) = verb else { return };
        if self.service.busy.is_some() {
            self.set_status(LogLevel::Warn, "a service command is still running");
            return;
        }
        if verb.needs_confirm() {
            self.confirm = Some(Confirm::Service(verb));
        } else {
            self.run_service_verb(verb);
        }
    }

    fn execute_confirmed(&mut self, c: Confirm) {
        match c {
            Confirm::DeleteTarget(i) => {
                if i < self.draft.cfg.targets.len() {
                    self.draft.cfg.targets.remove(i);
                    self.draft.dirty = true;
                }
                self.clamp_cursor();
                self.set_status(LogLevel::Info, "target removed");
            }
            Confirm::DeleteNotifier(i) => {
                if i < self.draft.cfg.notify.len() {
                    self.draft.cfg.notify.remove(i);
                    self.draft.dirty = true;
                }
                self.clamp_cursor();
                self.set_status(LogLevel::Info, "notifier removed");
            }
            Confirm::Reload => {
                self.draft.reload();
                self.clamp_cursor();
                self.set_status(LogLevel::Info, format!("reloaded {}", self.draft.path.display()));
            }
            Confirm::Service(v) => self.run_service_verb(v),
        }
    }

    /// Put the cursor on the row with this key (after adding an entry).
    fn jump_to(&mut self, key: RowKey) {
        if let Some(i) = self.rows().iter().position(|r| r.key == key) {
            *self.cursor_mut() = i;
        }
    }

    // ----- background work --------------------------------------------------------------

    fn fetch_for_current_row(&mut self) {
        let Some(row) = self.current_row() else { return };
        let (key, target) = match row.key {
            RowKey::Target(i, _) => (
                RowKey::Target(i, TargetField::Fingerprint),
                self.draft.cfg.targets[i].target.clone(),
            ),
            RowKey::Notifier(i, _) => (
                RowKey::Notifier(i, NotifierField::Fingerprint),
                self.draft.cfg.notify[i].url.clone(),
            ),
            _ => {
                self.set_status(LogLevel::Warn, "move onto a target or notifier to fetch its certificate");
                return;
            }
        };
        let (host, port, tls) = match row.key {
            RowKey::Target(..) => {
                let mut c = ClientConfig::default();
                if let Err(e) = crate::cli::args::parse_target(&target, &mut c) {
                    self.set_status(LogLevel::Error, e);
                    return;
                }
                let tls = c.protocol.is_tls() || c.protocol == nettest_proto::transport::Protocol::Https;
                (c.host, c.port, tls)
            }
            _ => match nettest_proto::http::Url::parse(&target) {
                Ok(u) => (u.host.clone(), u.port, u.https),
                Err(e) => {
                    self.set_status(LogLevel::Error, format!("url: {e}"));
                    return;
                }
            },
        };
        if !tls {
            self.set_status(LogLevel::Warn, format!("{target} is not a TLS endpoint (wss / https only)"));
            return;
        }
        self.fetch_fingerprint(key, host, port, None);
    }

    fn fetch_fingerprint(&mut self, key: RowKey, host: String, port: u16, ipv6: Option<bool>) {
        self.set_status(LogLevel::Info, format!("fetching the certificate of {host}:{port}..."));
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let result = nettest_proto::tls::client::peer_fingerprint(
                &host,
                port,
                ipv6,
                std::time::Duration::from_secs(10),
            )
            .await;
            let _ = tx.send(BgEvent::Fingerprint { key, result }).await;
        });
    }

    /// Notifiers of the draft, resolved; problems go to the pane.
    fn resolved_notifiers(&mut self, pane: Pane) -> Option<Vec<ResolvedNotifier>> {
        if self.draft.cfg.notify.is_empty() {
            self.set_status(LogLevel::Warn, "no webhooks configured (Alerts tab, a)");
            return None;
        }
        match config::resolve_notifiers(&self.draft.cfg.notify) {
            Ok((n, warnings)) => {
                for w in warnings {
                    self.push_result(pane, LogLevel::Warn, format!("warning: {w}"));
                }
                Some(n)
            }
            Err(errs) => {
                for e in errs {
                    self.push_result(pane, LogLevel::Error, e);
                }
                self.set_status(LogLevel::Error, "the notifier list has errors (see below)");
                None
            }
        }
    }

    fn test_notify(&mut self, pane: Pane) {
        let Some(notifiers) = self.resolved_notifiers(pane) else { return };
        let hostname = if self.cfg.alerts.hostname.trim().is_empty() {
            crate::monitor::local_hostname()
        } else {
            self.cfg.alerts.hostname.trim().to_string()
        };
        let msg = notify::test_notification(&hostname, self.draft.cfg.targets.len());
        let n = notifiers.len();
        self.send_notification(pane, &notifiers, msg);
        self.set_status(LogLevel::Info, format!("test message sent to {n} notifier(s), results below"));
    }

    fn send_notification(&mut self, pane: Pane, notifiers: &[ResolvedNotifier], msg: Notification) {
        for n in notifiers {
            let n = n.clone();
            let msg = msg.clone();
            let tx = self.bg_tx.clone();
            if pane == Pane::Run {
                self.alerts_in_flight += 1;
            }
            tokio::spawn(async move {
                let result = notify::deliver(&n, &msg).await;
                let _ = tx
                    .send(BgEvent::Notify {
                        pane,
                        name: format!("{} ({})", n.name, n.kind),
                        title: msg.title.clone(),
                        result,
                    })
                    .await;
            });
        }
    }

    fn verify_draft(&mut self) {
        self.builder_results.clear();
        if let Some(e) = &self.draft.load_error {
            let e = e.clone();
            self.push_result(Pane::Monitor, LogLevel::Error, e);
            return;
        }
        match config::resolve(&self.draft.cfg) {
            Ok(r) => {
                for w in &r.warnings {
                    self.push_result(Pane::Monitor, LogLevel::Warn, format!("warning     {w}"));
                }
                for l in config::describe(&r) {
                    self.push_result(Pane::Monitor, LogLevel::Info, l);
                }
                self.set_status(
                    LogLevel::Info,
                    format!(
                        "valid: {} target(s), {} notifier(s), {} warning(s)",
                        r.targets.len(),
                        r.notifiers.len(),
                        r.warnings.len()
                    ),
                );
            }
            Err(errs) => {
                let n = errs.len();
                for e in errs {
                    self.push_result(Pane::Monitor, LogLevel::Error, e);
                }
                self.set_status(LogLevel::Error, format!("{n} error(s), see below"));
            }
        }
    }

    pub fn probe_service(&mut self) {
        if self.service.probing {
            return;
        }
        self.service.probing = true;
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let st = tokio::task::spawn_blocking(service::status_probe).await;
            if let Ok(st) = st {
                let _ = tx.send(BgEvent::Status(st)).await;
            }
        });
    }

    fn run_service_verb(&mut self, verb: Verb) {
        let Some(st) = self.service.status.clone() else {
            self.set_status(LogLevel::Warn, "status not known yet; press R");
            return;
        };
        if !st.supported {
            self.set_status(LogLevel::Error, "no supported service manager on this host");
            return;
        }
        let from = PathBuf::from(self.service.from.trim());
        if verb == Verb::Install {
            // Fail here, in the TUI, rather than after a sudo / UAC prompt.
            match crate::monitor::cli::load_resolved(&from) {
                Ok(r) => {
                    for w in &r.warnings {
                        self.service.output.push((LogLevel::Warn, format!("warning     {w}")));
                    }
                }
                Err(errs) => {
                    self.service.output.push((
                        LogLevel::Error,
                        format!("{} is not a valid monitor.toml:", from.display()),
                    ));
                    for e in errs {
                        self.service.output.push((LogLevel::Error, format!("  - {e}")));
                    }
                    self.set_status(LogLevel::Error, "install refused: fix the config first (Monitor tab, v)");
                    return;
                }
            }
        }
        let args = verb.args((verb == Verb::Install).then_some(from.as_path()));
        let shown = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        self.service.output.push((LogLevel::Info, format!("$ nettest-client {shown}")));
        self.service.busy = Some(verb);
        if st.elevated {
            let tx = self.bg_tx.clone();
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || service::run_child(&args))
                    .await
                    .unwrap_or_else(|e| Err(format!("service task failed: {e}")));
                let _ = tx.send(BgEvent::ServiceDone { verb, result }).await;
            });
        } else if cfg!(windows) {
            let tx = self.bg_tx.clone();
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    nettest_service::elevate::reexec_elevated_captured(&args)
                        .map(|(code, text)| {
                            (if code == std::process::ExitCode::SUCCESS { 0 } else { 1 }, text)
                        })
                        .map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("service task failed: {e}")));
                let _ = tx.send(BgEvent::ServiceDone { verb, result }).await;
            });
        } else {
            // sudo needs the terminal: the event loop restores it, runs, and resumes.
            self.suspend = Some(args);
        }
    }

    /// Called by the event loop after a suspended (sudo) command returned.
    pub fn on_suspended_done(&mut self, ok: Option<bool>) {
        let verb = self.service.busy.take();
        let line = match (verb, ok) {
            (Some(v), Some(true)) => (LogLevel::Info, format!("{} finished (output was shown in the terminal)", v.label())),
            (Some(v), Some(false)) => (LogLevel::Error, format!("{} failed (see the terminal output above)", v.label())),
            (Some(v), None) => (LogLevel::Error, format!("{} could not be run", v.label())),
            (None, _) => (LogLevel::Info, "done".into()),
        };
        self.service.output.push(line);
        self.service.status = None;
        self.probe_service();
    }

    pub async fn on_bg(&mut self, ev: BgEvent) {
        match ev {
            BgEvent::Fingerprint { key, result } => match result {
                Ok((fp, addr)) => {
                    let hex = nettest_proto::tls::fingerprint::to_hex(&fp);
                    match key {
                        RowKey::Test(_) => self.cfg.fingerprint = hex.clone(),
                        RowKey::Target(i, _) => {
                            if let Some(t) = self.draft.cfg.targets.get_mut(i) {
                                t.fingerprint = hex.clone();
                                self.draft.dirty = true;
                            }
                        }
                        RowKey::Notifier(i, _) => {
                            if let Some(n) = self.draft.cfg.notify.get_mut(i) {
                                n.fingerprint = hex.clone();
                                self.draft.dirty = true;
                            }
                        }
                        _ => {}
                    }
                    self.set_status(
                        LogLevel::Warn,
                        format!(
                            "fingerprint from {addr}: {hex} - compare it with the one the server prints before trusting it"
                        ),
                    );
                }
                Err(e) => self.set_status(LogLevel::Error, format!("certificate fetch failed: {e}")),
            },
            BgEvent::Notify {
                pane,
                name,
                title,
                result,
            } => {
                if pane == Pane::Run {
                    self.alerts_in_flight = self.alerts_in_flight.saturating_sub(1);
                }
                match result {
                    Ok((status, attempts)) => self.push_result(
                        pane,
                        LogLevel::Info,
                        format!(
                            "notify {name}: delivered \"{title}\" ({status}{})",
                            if attempts > 1 { format!(", attempt {attempts}") } else { String::new() }
                        ),
                    ),
                    Err(e) => self.push_result(
                        pane,
                        LogLevel::Error,
                        format!("notify {name}: FAILED \"{title}\": {e}"),
                    ),
                }
            }
            BgEvent::Status(st) => {
                self.service.probing = false;
                self.service.status = Some(st);
                self.clamp_cursor();
            }
            BgEvent::ServiceDone { verb, result } => {
                self.service.busy = None;
                match result {
                    Ok((code, text)) => {
                        for l in text.lines() {
                            let level = if l.starts_with("error") || l.starts_with("warning") {
                                LogLevel::Warn
                            } else {
                                LogLevel::Info
                            };
                            self.service.output.push((level, l.to_string()));
                        }
                        if code == 0 {
                            self.service.output.push((LogLevel::Info, format!("{} ok", verb.label())));
                            self.set_status(LogLevel::Info, format!("service {} ok", verb.label()));
                        } else {
                            self.service.output.push((LogLevel::Error, format!("{} failed (exit code {code})", verb.label())));
                            self.set_status(LogLevel::Error, format!("service {} failed (exit code {code})", verb.label()));
                        }
                    }
                    Err(e) => {
                        self.service.output.push((LogLevel::Error, e.clone()));
                        self.set_status(LogLevel::Error, e);
                    }
                }
                self.service.status = None;
                self.probe_service();
            }
        }
    }

    /// 30 fps tick: reminder alerts while a run is DOWN.
    pub fn on_tick(&mut self) {
        if let Some(a) = self.run_alerts.as_mut()
            && let Some(msg) = a.on_tick()
        {
            let notifiers = a.notifiers.clone();
            self.send_notification(Pane::Run, &notifiers, msg);
        }
    }

    // ----- running / summary keys -------------------------------------------------------

    async fn on_key_running(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.request_quit().await,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('x') | KeyCode::Char('s') | KeyCode::Esc => {
                if let Some(tx) = &self.cmd_tx {
                    self.stop_requested = true;
                    let _ = tx.send(Command::Stop).await;
                    self.set_status(LogLevel::Info, "stopping...");
                }
            }
            KeyCode::Char('r') => self.export_report_now(),
            KeyCode::Char('c') => self.log.clear(),
            KeyCode::Char('1') => self.window = ChartWindow::Last60,
            KeyCode::Char('2') => self.window = ChartWindow::Last300,
            KeyCode::Char('3') => self.window = ChartWindow::All,
            _ => {}
        }
    }

    async fn on_key_summary(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('b') => {
                self.screen = Screen::Form;
                self.status = None;
            }
            KeyCode::Char('s') | KeyCode::F(5) => self.start().await,
            KeyCode::Char('r') => self.export_report_now(),
            KeyCode::Char('1') => self.window = ChartWindow::Last60,
            KeyCode::Char('2') => self.window = ChartWindow::Last300,
            KeyCode::Char('3') => self.window = ChartWindow::All,
            _ => {}
        }
    }

    // ----- runner lifecycle --------------------------------------------------------------

    async fn start(&mut self) {
        if self.is_running() {
            return;
        }
        if let Err(e) = form::preflight(&self.cfg) {
            self.set_status(LogLevel::Error, e);
            return;
        }
        let alerts = match RunAlerts::prepare(&self.cfg, &self.draft.cfg.notify) {
            Ok(a) => a,
            Err(errs) => {
                self.set_status(
                    LogLevel::Error,
                    format!(
                        "alerts not started: {} (fix it on the Alerts tab, or switch alerts off)",
                        errs.join("; ")
                    ),
                );
                return;
            }
        };
        let paths = OutputPaths::for_tui(&self.cfg);
        let mut sinks = MultiSink::new();
        match build_file_sinks(&self.cfg, &paths, &mut sinks) {
            Ok(opened) if !opened.is_empty() => {
                self.set_status(
                    LogLevel::Info,
                    format!(
                        "logging to {}",
                        opened
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            Ok(_) => self.status = None,
            Err(e) => {
                self.set_status(LogLevel::Error, format!("cannot open log file: {e}"));
                return;
            }
        }

        let (ev_tx, ev_rx) = mpsc::channel::<UiEvent>(4096);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(4);
        let runner = Runner::new(self.cfg.clone(), sinks, ev_tx, cmd_rx);
        self.join = Some(tokio::spawn(runner.run()));
        self.runner_rx = Some(ev_rx);
        self.cmd_tx = Some(cmd_tx);
        self.paths = Some(paths);
        self.run_cfg = self.cfg.clone();
        self.snapshot = Snapshot::default();
        self.link = Link::Idle;
        self.log.clear();
        self.events.clear();
        self.tp_series.clear();
        self.last_output = None;
        self.report_path = None;
        self.stop_requested = false;
        self.screen = Screen::Running;
        self.run_alerts = None;
        if let Some((a, warnings)) = alerts {
            for w in warnings {
                self.push_log(LogLevel::Warn, chrono::Local::now(), format!("alerts: {w}"));
            }
            self.push_log(
                LogLevel::Info,
                chrono::Local::now(),
                format!(
                    "alerts on: {} notifier(s), DOWN after {} failure(s), UP after {} success(es)",
                    a.notifiers.len(),
                    self.cfg.alerts.failures_before_down,
                    self.cfg.alerts.successes_before_up
                ),
            );
            if a.on_start {
                let msg = a.started(&self.cfg);
                let notifiers = a.notifiers.clone();
                self.send_notification(Pane::Run, &notifiers, msg);
            }
            self.run_alerts = Some(a);
        }
    }

    pub async fn on_runner_event(&mut self, ev: UiEvent) {
        if let Some(a) = self.run_alerts.as_mut()
            && let Some(msg) = a.on_event(&ev)
        {
            let notifiers = a.notifiers.clone();
            self.push_log(LogLevel::Warn, chrono::Local::now(), format!("alert: {}", msg.title));
            self.send_notification(Pane::Run, &notifiers, msg);
        }
        match ev {
            UiEvent::Connected(t) => {
                self.link = Link::Connected;
                self.set_status(
                    LogLevel::Info,
                    format!(
                        "connected to {} in {:.1} ms",
                        t.peer.map(|p| p.to_string()).unwrap_or_default(),
                        t.total.as_secs_f64() * 1e3
                    ),
                );
            }
            UiEvent::Disconnected(reason) => {
                self.link = Link::Reconnecting { attempt: 0 };
                self.set_status(LogLevel::Warn, format!("disconnected: {reason}"));
            }
            UiEvent::Reconnecting {
                attempt,
                backoff_ms,
                ..
            } => {
                self.link = Link::Reconnecting { attempt };
                self.set_status(
                    LogLevel::Warn,
                    format!("reconnect attempt {attempt} failed, retrying in {backoff_ms} ms"),
                );
            }
            UiEvent::Sample(s) => {
                if s.status.is_failure() || !self.run_cfg.sinks.console_failures_only {
                    let mut text = match s.rtt_ms {
                        Some(r) => format!("seq {} {} rtt {:.2} ms", s.seq, s.status.as_str(), r),
                        None => format!("seq {} {}", s.seq, s.status.as_str()),
                    };
                    if let Some(d) = &s.detail {
                        text.push_str(&format!(" ({d})"));
                    }
                    let level = if s.status.is_failure() {
                        LogLevel::Warn
                    } else {
                        LogLevel::Debug
                    };
                    self.push_log(level, s.sent_at, text);
                }
            }
            UiEvent::Snapshot(snap) => {
                if self.run_cfg.mode == TestMode::Throughput
                    && let Some(tp) = &snap.throughput
                {
                    self.tp_series.push((snap.elapsed_s, tp.instant_mbps));
                }
                self.snapshot = *snap;
                if !self.snapshot.connected && self.link == Link::Connected {
                    self.link = Link::Reconnecting { attempt: 0 };
                }
            }
            UiEvent::Log(e) => {
                self.push_log(e.level, e.at, e.describe());
                self.events.push(e);
            }
            UiEvent::Finished(summary) => {
                self.snapshot.latency = summary.latency.clone();
                self.snapshot.throughput = summary.throughput.clone();
                self.snapshot.soak = summary.soak.clone();
                if let Some(a) = self.run_alerts.take()
                    && a.on_finish
                {
                    let msg = a.finished(&summary);
                    self.send_notification(Pane::Run, &a.notifiers, msg);
                }
                self.finish().await;
            }
        }
    }

    async fn finish(&mut self) {
        self.link = Link::Idle;
        self.run_alerts = None;
        if let Some(j) = self.join.take()
            && let Ok(out) = j.await
        {
            if self.run_cfg.sinks.html_report
                && let Some(paths) = &self.paths
            {
                match write_report(&paths.report(), &out.summary, &out.chart, &out.events) {
                    Ok(p) => {
                        self.set_status(
                            LogLevel::Info,
                            format!("report written to {}", p.display()),
                        );
                        self.report_path = Some(p);
                    }
                    Err(e) => self.set_status(LogLevel::Error, format!("report failed: {e}")),
                }
            } else {
                self.status = None;
            }
            self.last_output = Some(out);
        }
        self.runner_rx = None;
        self.cmd_tx = None;
        if self.should_quit {
            return;
        }
        self.screen = Screen::Summary;
    }

    /// Write a report from whatever we have right now (mid-run or after).
    fn export_report_now(&mut self) {
        let (summary, chart, events): (RunSummary, Vec<ChartPoint>, Vec<Event>) =
            match &self.last_output {
                Some(out) => (out.summary.clone(), out.chart.clone(), out.events.clone()),
                None => (
                    RunSummary {
                        started_at: Some(chrono::Local::now()),
                        host: self.run_cfg.host.clone(),
                        port: self.run_cfg.port,
                        protocol: Some(self.run_cfg.protocol),
                        mode: self.run_cfg.mode.to_string(),
                        interval_ms: self.run_cfg.interval_ms,
                        payload_bytes: self.run_cfg.payload_bytes,
                        latency: self.snapshot.latency.clone(),
                        throughput: self.snapshot.throughput.clone(),
                        soak: self.snapshot.soak.clone(),
                        stop_reason: "in progress".into(),
                        ..Default::default()
                    },
                    self.snapshot.chart.clone(),
                    self.events.clone(),
                ),
            };
        let paths = self
            .paths
            .get_or_insert_with(|| OutputPaths::for_tui(&self.run_cfg));
        let path = paths.report();
        match write_report(&path, &summary, &chart, &events) {
            Ok(p) => {
                self.set_status(LogLevel::Info, format!("report written to {}", p.display()));
                self.report_path = Some(p);
            }
            Err(e) => self.set_status(LogLevel::Error, format!("report failed: {e}")),
        }
    }

    /// Test hook: arm the run alerts for `cfg` without launching a runner, so synthetic
    /// `UiEvent`s can be fed through `on_runner_event`.
    #[doc(hidden)]
    pub fn start_for_test(&mut self) {
        self.run_cfg = self.cfg.clone();
        self.run_alerts = RunAlerts::prepare(&self.cfg, &self.draft.cfg.notify)
            .expect("notifiers resolve")
            .map(|(a, _)| a);
    }

    #[doc(hidden)]
    pub fn is_running_alerts(&self) -> bool {
        self.run_alerts.is_some()
    }

    pub async fn shutdown(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Stop).await;
        }
        if let Some(j) = self.join.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), j).await;
        }
        // Give in-flight alert deliveries (DOWN/UP of a run that was just stopped) a moment.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while self.alerts_in_flight > 0 {
            match tokio::time::timeout_at(deadline, self.bg_rx.recv()).await {
                Ok(Some(ev)) => self.on_bg(ev).await,
                _ => break,
            }
        }
    }
}
