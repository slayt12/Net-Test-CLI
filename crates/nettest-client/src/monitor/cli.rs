//! `nettest-client monitor ...`: the foreground entry point and the helper flags
//! (`--check`, `--test-notify`, `--example-config`).

use std::path::PathBuf;
use std::process::ExitCode;

use nettest_proto::log::{LineLog, LogOptions};
use tokio_util::sync::CancellationToken;

use super::config::{self, Resolved};
use super::{RunOptions, notify};
use crate::cli::exit;

#[derive(clap::Args, Debug)]
pub struct MonitorArgs {
    /// monitor.toml to use (default: <user config dir>/nettest/monitor.toml)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Append the monitor log to this file (overrides [monitor].log_file)
    #[arg(long)]
    pub log_file: Option<PathBuf>,
    /// Validate the config, print the resolved targets and notifiers, and exit
    #[arg(long)]
    pub check: bool,
    /// Send a test message to every notifier and exit (0 = all delivered)
    #[arg(long)]
    pub test_notify: bool,
    /// Print an annotated example monitor.toml to stdout and exit
    #[arg(long)]
    pub example_config: bool,
}

pub fn default_config_path() -> PathBuf {
    nettest_proto::config::config_dir().join("monitor.toml")
}

/// Load + resolve, collecting every problem. Also used by `service status`; `service install`
/// goes through `read_text` itself because it needs the text to write the installed copy.
/// A file that is not plain UTF-8 is converted on the way in and reported as a warning so the
/// user knows what was found (`--check` output, the monitor log, the install report).
pub fn load_resolved(path: &std::path::Path) -> Result<Resolved, Vec<String>> {
    let (cfg, encoding) = config::load(path).map_err(|e| vec![e])?;
    let mut resolved = config::resolve(&cfg)?;
    resolved.encoding = encoding;
    if !encoding.is_utf8() {
        resolved.warnings.insert(0, config::encoding_warning(encoding));
    }
    Ok(resolved)
}

pub async fn run(args: MonitorArgs) -> ExitCode {
    if args.example_config {
        print!("{}", config::example_toml());
        return ExitCode::SUCCESS;
    }
    let path = args.config.clone().unwrap_or_else(default_config_path);
    let mut resolved = match load_resolved(&path) {
        Ok(r) => r,
        Err(errs) => {
            eprintln!("monitor config error ({}):", path.display());
            for e in errs {
                eprintln!("  - {e}");
            }
            return exit::code(exit::USAGE);
        }
    };
    if let Some(p) = &args.log_file {
        resolved.log_file = Some(p.clone());
    }

    if args.check {
        println!("config      {}", path.display());
        println!("encoding    {}", resolved.encoding);
        for l in config::describe(&resolved) {
            println!("{l}");
        }
        for w in &resolved.warnings {
            println!("warning     {w}");
        }
        return ExitCode::SUCCESS;
    }

    if args.test_notify {
        if resolved.notifiers.is_empty() {
            eprintln!("no [[notify]] entries in {}", path.display());
            return exit::code(exit::USAGE);
        }
        let (log, _tail) = match LineLog::start(LogOptions {
            stderr: true,
            text_file: None,
            jsonl_file: None,
            tail_capacity: 1,
        }) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("{e}");
                return exit::code(exit::IO);
            }
        };
        let msg = notify::test_notification(&resolved.hostname, resolved.targets.len());
        let failed = notify::deliver_all(&resolved.notifiers, &msg, &log).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        return if failed == 0 {
            ExitCode::SUCCESS
        } else {
            exit::code(exit::CONNECT)
        };
    }

    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    tokio::spawn(async move {
        super::shutdown_signal().await;
        signal.cancel();
    });
    let opts = RunOptions {
        stderr: true,
        log_file: resolved.log_file.clone(),
        jsonl_file: resolved.jsonl_file.clone(),
    };
    exit::code(super::run(resolved, opts, cancel).await)
}
