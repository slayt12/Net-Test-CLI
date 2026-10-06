//! Interactive terminal UI for the client.
//!
//! Architecture: the runner lives in its own tokio task and sends `UiEvent`s over a channel; this
//! module only renders state and translates keys into `Command`s. Rendering happens on a 30 fps
//! tick (plus after each key) rather than per event, so a 1 ms probe interval cannot starve the
//! terminal. Slow work (certificate fetches, webhook deliveries, service commands) reports back
//! over `App::bg_rx`.
//!
//! Invariants: the terminal is always restored, including on panic (see the hook in `run`).
//! A privileged service command on Linux runs with the terminal handed back to the shell
//! (sudo's password prompt needs it) and the crossterm reader dropped first, so it cannot
//! compete with sudo for keystrokes.

mod alerts;
mod app;
mod draft;
mod edit;
mod form;
mod rows;
mod views;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use nettest_proto::config::ClientConfig;

pub use app::App;

pub async fn run(cfg: ClientConfig, config_path: PathBuf) -> anyhow::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let monitor_path = crate::monitor::cli::default_config_path();
    let result = event_loop(App::new(cfg, config_path, monitor_path)).await;
    ratatui::restore();
    result
}

async fn event_loop(mut app: App) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    terminal.draw(|f| app.render(f))?;

    loop {
        let mut redraw = false;
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => {
                    app.on_key(k).await;
                    redraw = true;
                }
                Some(Ok(Event::Resize(_, _))) => redraw = true,
                Some(Ok(_)) => {}
                Some(Err(e)) => anyhow::bail!("terminal input error: {e}"),
                None => break,
            },
            Some(ue) = async { app.runner_rx.as_mut().unwrap().recv().await }, if app.runner_rx.is_some() => {
                app.on_runner_event(ue).await;
            }
            Some(bg) = app.bg_rx.recv() => {
                app.on_bg(bg).await;
                redraw = true;
            }
            _ = tick.tick() => {
                app.on_tick();
                redraw = true;
            }
        }
        if let Some(args) = app.suspend.take() {
            drop(input);
            ratatui::restore();
            let ok = tokio::task::block_in_place(|| run_in_terminal(&args));
            terminal = ratatui::init();
            input = EventStream::new();
            app.on_suspended_done(ok);
            redraw = true;
        }
        if app.should_quit {
            app.shutdown().await;
            break;
        }
        if redraw {
            terminal.draw(|f| app.render(f))?;
        }
    }
    Ok(())
}

/// Run `nettest-client service ...` through sudo / pkexec / doas with the terminal in its
/// normal state, then wait for Enter so the output can be read. `None` = could not be run.
fn run_in_terminal(args: &[OsString]) -> Option<bool> {
    let shown = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    println!("\n$ sudo nettest-client {shown}\n");
    let outcome = match nettest_service::elevate::reexec_elevated(args) {
        Ok(code) => {
            let ok = code == std::process::ExitCode::SUCCESS;
            println!("\n[{}]", if ok { "ok" } else { "failed" });
            Some(ok)
        }
        Err(e) => {
            println!("\n[could not run: {e}]");
            None
        }
    };
    println!("press Enter to return to nettest...");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    outcome
}

#[cfg(test)]
mod tests;
