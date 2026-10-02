//! Interactive terminal UI for the client.
//!
//! Architecture: the runner lives in its own tokio task and sends `UiEvent`s over a channel; this
//! module only renders state and translates keys into `Command`s. Rendering happens on a 30 fps
//! tick (plus after each key) rather than per event, so a 1 ms probe interval cannot starve the
//! terminal.
//!
//! Invariant: the terminal is always restored, including on panic (see the hook in `run`).

mod app;
mod form;
mod views;

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

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, App::new(cfg, config_path)).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> anyhow::Result<()> {
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
            _ = tick.tick() => redraw = true,
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

#[cfg(test)]
mod tests;
