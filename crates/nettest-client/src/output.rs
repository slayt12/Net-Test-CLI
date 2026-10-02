//! Output file naming and sink construction shared by the headless driver and the TUI.

use std::path::{Path, PathBuf};

use nettest_proto::config::ClientConfig;
use nettest_proto::report::HtmlReport;
use nettest_proto::sinks::{CsvSink, Event, JsonlSink, MultiSink, TextSink};
use nettest_proto::stats::{ChartPoint, RunSummary};

use crate::cli::args::Args;

pub struct OutputPaths {
    dir: PathBuf,
    stamp: String,
    csv: Option<String>,
    text: Option<String>,
    jsonl: Option<String>,
    report: Option<String>,
}

impl OutputPaths {
    pub fn new(cfg: &ClientConfig, args: &Args) -> Self {
        Self {
            dir: crate::cli::headless::default_out_dir(cfg),
            stamp: chrono::Local::now().format("%Y%m%d-%H%M%S").to_string(),
            csv: args.csv.clone(),
            text: args.text.clone(),
            jsonl: args.jsonl.clone(),
            report: args.report.clone(),
        }
    }

    pub fn for_tui(cfg: &ClientConfig) -> Self {
        Self {
            dir: crate::cli::headless::default_out_dir(cfg),
            stamp: chrono::Local::now().format("%Y%m%d-%H%M%S").to_string(),
            csv: None,
            text: None,
            jsonl: None,
            report: None,
        }
    }

    fn pick(&self, explicit: &Option<String>, ext: &str) -> PathBuf {
        match explicit {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => self.dir.join(format!("nettest-{}.{ext}", self.stamp)),
        }
    }

    pub fn csv(&self) -> PathBuf {
        self.pick(&self.csv, "csv")
    }
    pub fn text(&self) -> PathBuf {
        self.pick(&self.text, "log")
    }
    pub fn jsonl(&self) -> PathBuf {
        self.pick(&self.jsonl, "jsonl")
    }
    pub fn report(&self) -> PathBuf {
        self.pick(&self.report, "html")
    }
}

pub fn build_file_sinks(
    cfg: &ClientConfig,
    paths: &OutputPaths,
    sinks: &mut MultiSink,
) -> std::io::Result<Vec<PathBuf>> {
    let mut opened = Vec::new();
    if cfg.sinks.csv || cfg.sinks.text || cfg.sinks.jsonl || cfg.sinks.html_report {
        std::fs::create_dir_all(crate::cli::headless::default_out_dir(cfg))?;
    }
    if cfg.sinks.csv {
        let p = paths.csv();
        sinks.push(Box::new(CsvSink::create(&p)?));
        opened.push(p);
    }
    if cfg.sinks.text {
        let p = paths.text();
        sinks.push(Box::new(TextSink::create(
            &p,
            cfg.sinks.console_failures_only,
        )?));
        opened.push(p);
    }
    if cfg.sinks.jsonl {
        let p = paths.jsonl();
        sinks.push(Box::new(JsonlSink::create(&p)?));
        opened.push(p);
    }
    Ok(opened)
}

pub fn write_report(
    path: &Path,
    summary: &RunSummary,
    chart: &[ChartPoint],
    events: &[Event],
) -> std::io::Result<PathBuf> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, HtmlReport::build(summary, chart, events))?;
    Ok(path.to_path_buf())
}
