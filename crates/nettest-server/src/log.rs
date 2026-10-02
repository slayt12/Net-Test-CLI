//! Server log pipeline: producers call `ServerLog::{info,warn,error}`; a single consumer task
//! writes to stderr and/or files and keeps a bounded in-memory tail for the TUI.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use nettest_proto::sinks::LogLevel;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: chrono::DateTime<chrono::Local>,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone)]
pub struct ServerLog {
    tx: mpsc::UnboundedSender<LogLine>,
}

pub type LogTail = Arc<Mutex<VecDeque<LogLine>>>;

pub struct LogOptions {
    pub stderr: bool,
    pub text_file: Option<String>,
    pub jsonl_file: Option<String>,
    pub tail_capacity: usize,
}

impl ServerLog {
    /// Returns the handle plus the shared tail the TUI renders from.
    pub fn start(opts: LogOptions) -> std::io::Result<(ServerLog, LogTail)> {
        let (tx, mut rx) = mpsc::unbounded_channel::<LogLine>();
        let tail: LogTail = Arc::new(Mutex::new(VecDeque::with_capacity(opts.tail_capacity)));
        let mut text = match &opts.text_file {
            Some(p) if !p.is_empty() => Some(BufWriter::new(open_append(Path::new(p))?)),
            _ => None,
        };
        let mut jsonl = match &opts.jsonl_file {
            Some(p) if !p.is_empty() => Some(BufWriter::new(open_append(Path::new(p))?)),
            _ => None,
        };
        let tail2 = tail.clone();
        let cap = opts.tail_capacity.max(1);
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let stamp = line.at.format("%Y-%m-%d %H:%M:%S%.3f");
                if opts.stderr {
                    eprintln!("{stamp} {:<5} {}", line.level.as_str(), line.text);
                }
                if let Some(w) = text.as_mut() {
                    let _ = writeln!(w, "{stamp} {:<5} {}", line.level.as_str(), line.text);
                    let _ = w.flush();
                }
                if let Some(w) = jsonl.as_mut() {
                    let _ = writeln!(
                        w,
                        "{{\"at\":\"{}\",\"level\":\"{}\",\"text\":{}}}",
                        line.at.to_rfc3339(),
                        line.level.as_str().to_ascii_lowercase(),
                        serde_json::to_string(&line.text).unwrap_or_default()
                    );
                    let _ = w.flush();
                }
                if let Ok(mut t) = tail2.lock() {
                    if t.len() == cap {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            }
        });
        Ok((ServerLog { tx }, tail))
    }

    pub fn log(&self, level: LogLevel, text: impl Into<String>) {
        let _ = self.tx.send(LogLine {
            at: chrono::Local::now(),
            level,
            text: text.into(),
        });
    }
    pub fn info(&self, text: impl Into<String>) {
        self.log(LogLevel::Info, text);
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.log(LogLevel::Warn, text);
    }
    pub fn error(&self, text: impl Into<String>) {
        self.log(LogLevel::Error, text);
    }
}

fn open_append(p: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
}
