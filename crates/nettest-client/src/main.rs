//! nettest-client entry point: parse flags, then run the interactive TUI, the headless driver,
//! the endpoint monitor (`monitor`) or a service-management command (`service`).
//!
//! A plain `fn main` (not `#[tokio::main]`): on Windows the Service Control Manager calls the
//! service entry on its own thread and that thread must own the runtime, so the runtime is built
//! explicitly for the other paths. Exit codes are documented in `cli::exit`.

use std::process::ExitCode;

use clap::Parser;
use nettest_client::cli::args::{Args, Cmd};
use nettest_client::cli::exit;
use nettest_client::{cli, monitor, service, tui};

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Some(Cmd::Service(s)) => service::dispatch(s),
        Some(Cmd::Monitor(m)) => with_runtime(|| monitor::cli::run(m)),
        None => with_runtime(|| classic(args)),
    }
}

fn with_runtime<F: std::future::Future<Output = ExitCode>>(f: impl FnOnce() -> F) -> ExitCode {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("could not start the async runtime: {e}");
            return exit::code(exit::USAGE);
        }
    };
    rt.block_on(f())
}

/// The original single-target client: saved config + flags, then TUI or headless.
async fn classic(args: Args) -> ExitCode {
    let path = args
        .config
        .clone()
        .unwrap_or_else(nettest_proto::config::client_config_path);
    let saved = match nettest_proto::config::load::<nettest_proto::config::ClientConfig>(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return exit::code(exit::USAGE);
        }
    };
    let cfg = match args.apply(saved) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::code(exit::USAGE);
        }
    };
    if args.save_config
        && let Err(e) = nettest_proto::config::save(&path, &cfg)
    {
        eprintln!("could not save config: {e}");
        return exit::code(exit::USAGE);
    }

    if args.no_tui {
        return exit::code(cli::headless::run(&args, cfg).await);
    }
    match tui::run(cfg, path).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            exit::code(exit::IO)
        }
    }
}
