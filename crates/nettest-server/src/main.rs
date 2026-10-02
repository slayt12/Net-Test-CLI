//! nettest-server entry point.
//!
//! A plain `fn main` (not `#[tokio::main]`): the Windows Service Control Manager calls the
//! service entry on its own thread, and that thread has to own the runtime. Foreground runs
//! build the runtime here instead.
//!
//! Exit codes: 0 clean shutdown, 1 TUI error, 2 could not bind / start, 3 bad arguments, config
//! or log file. `service` subcommands: 0 ok, 2 service manager failure, 3 usage / not supported /
//! elevation declined / missing token.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use nettest_proto::config::ServerConfig;
use nettest_server::log::{LogOptions, ServerLog};

mod cli;
mod service;
mod tui;

use cli::args::{Args, Cmd, ServerFlags};

fn main() -> ExitCode {
    let args = Args::parse();
    if let Some(Cmd::Service(s)) = args.command {
        return service::dispatch(s);
    }
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not start the async runtime: {e}");
            return ExitCode::from(3);
        }
    };
    rt.block_on(foreground(args))
}

/// Config file (or defaults) with flags applied on top. Shared by foreground and service runs.
pub fn load_config(
    path: Option<&Path>,
    flags: &ServerFlags,
) -> Result<(PathBuf, ServerConfig), String> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(nettest_proto::config::server_config_path);
    let cfg = nettest_proto::config::load::<ServerConfig>(&path)
        .map_err(|e| format!("config error: {e}"))?;
    Ok((path, flags.apply(cfg)))
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

async fn foreground(args: Args) -> ExitCode {
    let (path, cfg) = match load_config(args.config.as_deref(), &args.flags) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(3);
        }
    };
    if args.save_config
        && let Err(e) = nettest_proto::config::save(&path, &cfg)
    {
        eprintln!("could not save config: {e}");
        return ExitCode::from(3);
    }

    let (log, tail) = match ServerLog::start(LogOptions {
        stderr: args.no_tui,
        text_file: Some(cfg.log_file.clone()),
        jsonl_file: Some(cfg.jsonl_file.clone()),
        tail_capacity: 500,
    }) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("could not open log file: {e}");
            return ExitCode::from(3);
        }
    };

    let server = match nettest_server::start(cfg.clone(), log.clone()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("startup failed: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(fp) = &server.fingerprint {
        log.info(format!(
            "wss certificate fingerprint (SHA-256): {}",
            nettest_proto::tls::fingerprint::to_hex(fp)
        ));
    }
    if cfg.token.is_empty() {
        log.warn("no token configured: any client can use this server (set --token)");
    } else {
        log.info("token authentication enabled");
    }

    let outcome = if args.no_tui {
        log.info("press Ctrl-C to stop");
        tokio::select! {
            _ = shutdown_signal() => Ok(()),
            _ = server.ctx.cancel.cancelled() => Ok(()),
        }
    } else {
        tui::run(&server, tail, path).await
    };

    server.shutdown();
    log.info("shutting down");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
