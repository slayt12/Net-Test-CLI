//! Headless driver: runs the test with no terminal UI, prints a summary, returns an exit code.
//! Used by scripts, cron and remote shells where a TUI is unwanted or impossible.

use std::path::PathBuf;

use nettest_proto::config::ClientConfig;
use nettest_proto::sinks::{ConsoleSink, MultiSink};
use nettest_proto::stats::RunSummary;
use tokio::sync::mpsc;

use super::args::Args;
use super::exit;
use crate::output::{OutputPaths, build_file_sinks, write_report};
use crate::runner::{Command, Runner, StopReason, UiEvent};

pub async fn run(args: &Args, cfg: ClientConfig) -> u8 {
    let paths = OutputPaths::new(&cfg, args);
    let mut sinks = MultiSink::new();
    if cfg.sinks.console {
        sinks.push(Box::new(ConsoleSink::new(cfg.sinks.console_failures_only)));
    }
    if let Err(e) = build_file_sinks(&cfg, &paths, &mut sinks) {
        eprintln!("cannot open output file: {e}");
        return exit::IO;
    }

    let (ev_tx, mut ev_rx) = mpsc::channel::<UiEvent>(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(4);
    let runner = Runner::new(cfg.clone(), sinks, ev_tx, cmd_rx);
    let handle = tokio::spawn(runner.run());

    let mut interrupted = false;
    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _ = &mut ctrl_c, if !interrupted => {
                interrupted = true;
                eprintln!("interrupted, stopping...");
                let _ = cmd_tx.send(Command::Stop).await;
            }
            ev = ev_rx.recv() => match ev {
                Some(UiEvent::Finished(_)) | None => break,
                Some(_) => {}
            }
        }
    }
    let out = match handle.await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("runner panicked: {e}");
            return exit::IO;
        }
    };

    let mut io_failed = false;
    if cfg.sinks.html_report {
        match write_report(&paths.report(), &out.summary, &out.chart, &out.events) {
            Ok(p) => eprintln!("report written to {}", p.display()),
            Err(e) => {
                eprintln!("could not write report: {e}");
                io_failed = true;
            }
        }
    }

    if args.json_summary {
        println!(
            "{}",
            serde_json::to_string(&out.summary).unwrap_or_default()
        );
    } else {
        print_summary(&out.summary);
    }

    let bounded = cfg.count > 0 || cfg.duration_secs > 0;
    match out.reason {
        StopReason::ConnectFailed(_) => exit::CONNECT,
        StopReason::Stopped if interrupted && bounded => exit::INTERRUPTED,
        _ if io_failed => exit::IO,
        _ => {
            let l = &out.summary.latency;
            let loss_bad = l.sent > 0 && l.loss_pct > args.max_loss + f64::EPSILON;
            let p95_bad = args
                .max_p95_ms
                .is_some_and(|m| l.received > 0 && l.p95_ms > m);
            if loss_bad || p95_bad {
                exit::THRESHOLD
            } else {
                exit::OK
            }
        }
    }
}

pub fn print_summary(s: &RunSummary) {
    let l = &s.latency;
    println!("--- nettest summary ---");
    println!(
        "target     {}  mode {}  ({})",
        target_of(s),
        s.mode,
        s.stop_reason
    );
    if l.sent > 0 {
        println!(
            "probes     sent {}  received {}  lost {} ({:.2}%){}  late {}  ooo {}  dup {}",
            l.sent,
            l.received,
            l.lost,
            l.loss_pct,
            if l.failed > 0 {
                format!("  failed {}", l.failed)
            } else {
                String::new()
            },
            l.late,
            l.out_of_order,
            l.duplicates
        );
        println!(
            "rtt ms     min {:.2}  avg {:.2}  max {:.2}  p50 {:.2}  p95 {:.2}  p99 {:.2}  jitter {:.2}",
            l.min_ms, l.avg_ms, l.max_ms, l.p50_ms, l.p95_ms, l.p99_ms, l.jitter_ms
        );
    }
    if let Some(t) = &s.throughput {
        if t.up_bytes > 0 {
            println!(
                "upload     {:.2} MB in {:.2}s = {:.2} Mbit/s ({} frames)",
                t.up_bytes as f64 / 1e6,
                t.up_secs,
                t.up_mbps,
                t.up_frames
            );
        }
        if t.down_bytes > 0 {
            println!(
                "download   {:.2} MB in {:.2}s = {:.2} Mbit/s ({} frames{})",
                t.down_bytes as f64 / 1e6,
                t.down_secs,
                t.down_mbps,
                t.down_frames,
                if t.down_expected_frames > 0 {
                    format!(", {:.2}% lost", t.down_loss_pct)
                } else {
                    String::new()
                }
            );
        }
    }
    if let Some(k) = &s.soak {
        println!(
            "stability  disconnects {}  reconnects {}  uptime {:.3}%  longest outage {} ms  longest session {:.0}s",
            k.disconnects, k.reconnects, k.uptime_pct, k.longest_outage_ms, k.longest_session_s
        );
    }
    if let Some(c) = s.connects.first() {
        let ms = |d: Option<std::time::Duration>| {
            d.map(|d| format!("{:.1}", d.as_secs_f64() * 1e3))
                .unwrap_or_else(|| "-".into())
        };
        println!(
            "connect ms dns {}  tcp {}  tls {}  ws {}  hello {}  total {:.1}  ({} connects)",
            ms(c.dns),
            ms(c.tcp),
            ms(c.tls),
            ms(c.ws_upgrade),
            ms(c.hello),
            c.total.as_secs_f64() * 1e3,
            s.connects.len()
        );
    }
}

/// `scheme://host:port`, without the meaningless port for ping.
fn target_of(s: &RunSummary) -> String {
    let scheme = s.protocol.map(|p| p.to_string()).unwrap_or_default();
    if s.protocol == Some(nettest_proto::transport::Protocol::Ping) {
        format!("{scheme}://{}", s.host)
    } else {
        format!("{scheme}://{}:{}", s.host, s.port)
    }
}

pub fn default_out_dir(cfg: &ClientConfig) -> PathBuf {
    PathBuf::from(if cfg.sinks.output_dir.is_empty() {
        "."
    } else {
        cfg.sinks.output_dir.as_str()
    })
}
