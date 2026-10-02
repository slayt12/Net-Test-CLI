//! nettest-client entry point: parse flags, merge with the saved config, then run either the
//! interactive TUI or the headless driver. Exit codes are documented in `cli::exit`.

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod output;
mod runner;
mod tui;

#[tokio::main]
async fn main() -> ExitCode {
    let args = cli::args::Args::parse();
    let path = args
        .config
        .clone()
        .unwrap_or_else(nettest_proto::config::client_config_path);
    let saved = match nettest_proto::config::load::<nettest_proto::config::ClientConfig>(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return cli::exit::code(cli::exit::USAGE);
        }
    };
    let cfg = match args.apply(saved) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return cli::exit::code(cli::exit::USAGE);
        }
    };
    if args.save_config
        && let Err(e) = nettest_proto::config::save(&path, &cfg)
    {
        eprintln!("could not save config: {e}");
        return cli::exit::code(cli::exit::USAGE);
    }

    if args.no_tui {
        return cli::exit::code(cli::headless::run(&args, cfg).await);
    }
    match tui::run(cfg, path).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            cli::exit::code(cli::exit::IO)
        }
    }
}
