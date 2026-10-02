//! Self-contained HTML report: summary tables plus a hand-rolled inline SVG chart.
//!
//! No JavaScript and no external resources, so the file can be attached to a ticket and opened
//! on an air-gapped machine without anything leaking or breaking.

mod escape;
mod svg;

pub use escape::html_escape;

use crate::sinks::Event;
use crate::stats::{ChartPoint, RunSummary};
use crate::transport::ConnectTimings;

pub struct HtmlReport;

impl HtmlReport {
    pub fn build(summary: &RunSummary, points: &[ChartPoint], events: &[Event]) -> String {
        let mut html = String::with_capacity(16 * 1024 + points.len() * 16);
        html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
        html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
        html.push_str("<title>nettest report</title><style>");
        html.push_str(CSS);
        html.push_str("</style></head><body><main>");

        let port = if summary.protocol == Some(crate::transport::Protocol::Ping) {
            String::new()
        } else {
            format!(":{}", summary.port)
        };
        html.push_str(&format!(
            "<h1>nettest report</h1><p class=\"sub\">{} &rarr; {}{} over {} &middot; {} mode &middot; {}</p>",
            html_escape(&summary.started_at.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default()),
            html_escape(&summary.host),
            port,
            summary.protocol.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            html_escape(&summary.mode),
            html_escape(&summary.stop_reason),
        ));

        let l = &summary.latency;
        html.push_str("<section class=\"tiles\">");
        for (label, value) in [
            ("Sent", summary.latency.sent.to_string()),
            ("Received", l.received.to_string()),
            (
                "Lost",
                if l.failed > 0 {
                    format!("{} ({:.2}%), {} failed", l.lost, l.loss_pct, l.failed)
                } else {
                    format!("{} ({:.2}%)", l.lost, l.loss_pct)
                },
            ),
            ("Avg RTT", format!("{:.2} ms", l.avg_ms)),
            (
                "p50 / p95 / p99",
                format!("{:.1} / {:.1} / {:.1} ms", l.p50_ms, l.p95_ms, l.p99_ms),
            ),
            ("Min / Max", format!("{:.2} / {:.2} ms", l.min_ms, l.max_ms)),
            ("Jitter", format!("{:.2} ms", l.jitter_ms)),
            (
                "Out of order / Dup / Late",
                format!("{} / {} / {}", l.out_of_order, l.duplicates, l.late),
            ),
            (
                "Duration",
                humantime::format_duration(std::time::Duration::from_secs(l.elapsed.as_secs()))
                    .to_string(),
            ),
        ] {
            html.push_str(&format!(
                "<div class=\"tile\"><div class=\"k\">{}</div><div class=\"v\">{}</div></div>",
                html_escape(label),
                html_escape(&value)
            ));
        }
        html.push_str("</section>");

        if !points.is_empty() {
            html.push_str("<h2>Round-trip time</h2><div class=\"chart\">");
            html.push_str(&svg::rtt_chart(points, 900.0, 300.0));
            html.push_str("</div><p class=\"legend\"><span class=\"sw rtt\"></span>RTT (ms) <span class=\"sw loss\"></span>lost probe</p>");
        }

        if let Some(tp) = &summary.throughput {
            html.push_str("<h2>Throughput</h2><table><thead><tr><th>Direction</th><th>Bytes</th><th>Seconds</th><th>Mbit/s</th><th>Frames</th></tr></thead><tbody>");
            if tp.up_bytes > 0 {
                html.push_str(&format!(
                    "<tr><td>upload</td><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{}</td></tr>",
                    tp.up_bytes, tp.up_secs, tp.up_mbps, tp.up_frames
                ));
            }
            if tp.down_bytes > 0 {
                html.push_str(&format!(
                    "<tr><td>download</td><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{}</td></tr>",
                    tp.down_bytes, tp.down_secs, tp.down_mbps, tp.down_frames
                ));
            }
            html.push_str("</tbody></table>");
        }

        if let Some(soak) = &summary.soak {
            html.push_str(&format!(
                "<h2>Connection stability</h2><p>{} disconnect(s), {} reconnect(s), uptime {:.3}%, longest outage {} ms, longest session {:.0} s.</p>",
                soak.disconnects, soak.reconnects, soak.uptime_pct, soak.longest_outage_ms, soak.longest_session_s
            ));
            if !soak.events.is_empty() {
                html.push_str("<table><thead><tr><th>Time</th><th>Reason</th><th>Attempts</th><th>Downtime</th></tr></thead><tbody>");
                for e in &soak.events {
                    html.push_str(&format!(
                        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                        e.at.format("%H:%M:%S"),
                        html_escape(&e.reason),
                        e.reconnect_attempts,
                        e.downtime_ms
                            .map(|d| format!("{d} ms"))
                            .unwrap_or_else(|| "ongoing".into())
                    ));
                }
                html.push_str("</tbody></table>");
            }
        }

        if !summary.connects.is_empty() {
            html.push_str("<h2>Connect phases</h2><table><thead><tr><th>#</th><th>Peer</th><th>DNS</th><th>TCP</th><th>TLS</th><th>WS upgrade</th><th>Hello</th><th>Total</th></tr></thead><tbody>");
            for (i, c) in summary.connects.iter().enumerate() {
                html.push_str(&connect_row(i + 1, c));
            }
            html.push_str("</tbody></table>");
        }

        if !events.is_empty() {
            html.push_str("<h2>Events</h2><table><thead><tr><th>Time</th><th>Level</th><th>Event</th></tr></thead><tbody>");
            for e in events {
                html.push_str(&format!(
                    "<tr><td>{}</td><td class=\"lvl-{}\">{}</td><td>{}</td></tr>",
                    e.at.format("%H:%M:%S%.3f"),
                    e.level.as_str().to_ascii_lowercase(),
                    e.level.as_str(),
                    html_escape(&e.describe())
                ));
            }
            html.push_str("</tbody></table>");
        }

        html.push_str(
            "<footer>Generated by nettest. Single-file report, no external resources.</footer>",
        );
        html.push_str("</main></body></html>");
        html
    }
}

fn ms(d: Option<std::time::Duration>) -> String {
    d.map(|d| format!("{:.1} ms", d.as_secs_f64() * 1e3))
        .unwrap_or_else(|| "&ndash;".into())
}

fn connect_row(i: usize, c: &ConnectTimings) -> String {
    format!(
        "<tr><td>{i}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
        c.peer.map(|p| p.to_string()).unwrap_or_default(),
        ms(c.dns),
        ms(c.tcp),
        ms(c.tls),
        ms(c.ws_upgrade),
        ms(c.hello),
        ms(Some(c.total)),
    )
}

const CSS: &str = r#"
:root{color-scheme:light dark;--bg:#fafafa;--fg:#1a1a1a;--muted:#666;--line:#ddd;--card:#fff;--rtt:#2563eb;--loss:#dc2626}
@media(prefers-color-scheme:dark){:root{--bg:#121212;--fg:#eee;--muted:#999;--line:#333;--card:#1c1c1c;--rtt:#60a5fa;--loss:#f87171}}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:960px;margin:0 auto;padding:24px 16px}
h1{font-size:22px;margin:0 0 4px}h2{font-size:16px;margin:28px 0 8px}
.sub{color:var(--muted);margin:0 0 20px}
.tiles{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:10px}
.tile{background:var(--card);border:1px solid var(--line);border-radius:6px;padding:10px 12px}
.tile .k{font-size:11px;text-transform:uppercase;letter-spacing:.04em;color:var(--muted)}
.tile .v{font-size:17px;font-variant-numeric:tabular-nums;margin-top:2px}
.chart{background:var(--card);border:1px solid var(--line);border-radius:6px;padding:8px;overflow-x:auto}
svg{display:block;width:100%;height:auto;max-width:100%}
.axis{stroke:var(--line)}.grid{stroke:var(--line);stroke-dasharray:2 4}
.rtt-line{fill:none;stroke:var(--rtt);stroke-width:1.5}
.loss-dot{fill:var(--loss)}
.tick{fill:var(--muted);font-size:11px}
.legend{color:var(--muted);font-size:12px}
.sw{display:inline-block;width:10px;height:10px;border-radius:2px;margin:0 6px 0 10px;vertical-align:middle}
.sw.rtt{background:var(--rtt)}.sw.loss{background:var(--loss)}
table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line)}
th{font-size:11px;text-transform:uppercase;letter-spacing:.04em;color:var(--muted)}
.lvl-warn{color:#b45309}.lvl-error{color:var(--loss)}
footer{margin-top:36px;color:var(--muted);font-size:12px}
"#;
