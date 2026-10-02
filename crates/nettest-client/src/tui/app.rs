//! Application state and key handling. Rendering is delegated to `views`.

use std::collections::VecDeque;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nettest_proto::config::{ClientConfig, TestMode};
use nettest_proto::sinks::{Event, LogLevel, MultiSink};
use nettest_proto::stats::{ChartPoint, RunSummary};
use ratatui::Frame;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::form::{self, FIELDS, FieldKind};
use crate::output::{OutputPaths, build_file_sinks, write_report};
use crate::runner::{Command, RunOutput, Runner, Snapshot, UiEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Form,
    Running,
    Summary,
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

pub struct App {
    pub cfg: ClientConfig,
    pub config_path: PathBuf,
    pub screen: Screen,
    pub should_quit: bool,
    pub show_help: bool,

    // Form
    pub field: usize,
    pub editing: Option<String>,
    pub status: Option<(LogLevel, String)>,

    // Run
    pub runner_rx: Option<mpsc::Receiver<UiEvent>>,
    cmd_tx: Option<mpsc::Sender<Command>>,
    join: Option<JoinHandle<RunOutput>>,
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

impl App {
    pub fn new(cfg: ClientConfig, config_path: PathBuf) -> Self {
        Self {
            run_cfg: cfg.clone(),
            cfg,
            config_path,
            screen: Screen::Form,
            should_quit: false,
            show_help: false,
            field: 0,
            editing: None,
            status: None,
            runner_rx: None,
            cmd_tx: None,
            join: None,
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
        if let Some(buf) = self.editing.as_mut() {
            match k.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    let text = buf.clone();
                    let id = FIELDS[self.field].id;
                    match form::commit(&mut self.cfg, id, &text) {
                        Ok(()) => {
                            self.editing = None;
                            self.status = None;
                        }
                        Err(e) => self.set_status(LogLevel::Error, e),
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => buf.push(c),
                _ => {}
            }
            return;
        }
        let def = &FIELDS[self.field];
        match k.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.field = (self.field + 1) % FIELDS.len();
            }
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.field = (self.field + FIELDS.len() - 1) % FIELDS.len();
            }
            KeyCode::Home => self.field = 0,
            KeyCode::End => self.field = FIELDS.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') if def.kind != FieldKind::Text => {
                form::cycle(&mut self.cfg, def.id);
            }
            KeyCode::Enter => {
                self.editing = Some(form::value_of(&self.cfg, def.id));
            }
            KeyCode::Char('w') => match nettest_proto::config::save(&self.config_path, &self.cfg) {
                Ok(()) => self.set_status(
                    LogLevel::Info,
                    format!("settings saved to {}", self.config_path.display()),
                ),
                Err(e) => self.set_status(LogLevel::Error, format!("save failed: {e}")),
            },
            KeyCode::Char('s') | KeyCode::F(5) => self.start().await,
            _ => {}
        }
    }

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
    }

    pub async fn on_runner_event(&mut self, ev: UiEvent) {
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
                self.finish().await;
            }
        }
    }

    async fn finish(&mut self) {
        self.link = Link::Idle;
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

    pub async fn shutdown(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Stop).await;
        }
        if let Some(j) = self.join.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), j).await;
        }
    }
}
