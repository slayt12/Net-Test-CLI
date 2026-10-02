//! Live chart: RTT line with loss markers, or instant throughput in throughput mode.

use nettest_proto::stats::ChartPoint;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::text::Span;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};

use crate::tui::app::{App, ChartWindow};

pub fn render_rtt(app: &App, f: &mut Frame, area: Rect) {
    let elapsed = app.snapshot.elapsed_s.max(1.0);
    let (x0, x1) = window_bounds(app.window, elapsed);
    let pts: Vec<&ChartPoint> = app.snapshot.chart.iter().filter(|p| p.t >= x0).collect();
    let rtt: Vec<(f64, f64)> = pts
        .iter()
        .filter_map(|p| p.rtt_ms.map(|r| (p.t, r)))
        .collect();
    let y_max = nice_ceiling(
        rtt.iter()
            .map(|(_, r)| *r)
            .fold(0.0_f64, f64::max)
            .max(app.snapshot.latency.p99_ms * 1.2)
            .max(1.0),
    );
    let losses: Vec<(f64, f64)> = pts
        .iter()
        .filter(|p| p.rtt_ms.is_none())
        .map(|p| (p.t, y_max * 0.02))
        .collect();

    let mut datasets = vec![
        Dataset::default()
            .name("rtt ms")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(&rtt),
    ];
    if !losses.is_empty() {
        datasets.push(
            Dataset::default()
                .name("lost")
                .marker(symbols::Marker::Block)
                .graph_type(GraphType::Scatter)
                .style(Style::default().fg(Color::Red))
                .data(&losses),
        );
    }

    let title = format!(" RTT  [{}]  1/2/3 window ", app.window.label());
    let chart = Chart::new(datasets)
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .bounds([x0, x1])
                .labels(x_labels(x0, x1))
                .style(Style::default().fg(Color::DarkGray)),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(fmt_axis(y_max / 2.0)),
                    Span::raw(fmt_axis(y_max)),
                ])
                .style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(chart, area);
}

pub fn render_throughput(app: &App, f: &mut Frame, area: Rect) {
    let elapsed = app.snapshot.elapsed_s.max(1.0);
    let (x0, x1) = window_bounds(app.window, elapsed);
    let series: Vec<(f64, f64)> = app
        .tp_series
        .iter()
        .filter(|(t, _)| *t >= x0)
        .copied()
        .collect();
    let y_max = nice_ceiling(
        series
            .iter()
            .map(|(_, v)| *v)
            .fold(0.0_f64, f64::max)
            .max(1.0),
    );
    let ds = vec![
        Dataset::default()
            .name("Mbit/s")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Green))
            .data(&series),
    ];
    let chart = Chart::new(ds)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Throughput (1 s window) "),
        )
        .x_axis(
            Axis::default()
                .bounds([x0, x1])
                .labels(x_labels(x0, x1))
                .style(Style::default().fg(Color::DarkGray)),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("0"),
                    Span::raw(fmt_axis(y_max / 2.0)),
                    Span::raw(fmt_axis(y_max)),
                ])
                .style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(chart, area);
}

fn window_bounds(w: ChartWindow, elapsed: f64) -> (f64, f64) {
    match w.secs() {
        Some(s) => ((elapsed - s).max(0.0), elapsed.max(s.min(elapsed).max(1.0))),
        None => (0.0, elapsed),
    }
}

fn x_labels(x0: f64, x1: f64) -> Vec<Span<'static>> {
    let mid = (x0 + x1) / 2.0;
    vec![
        Span::raw(super::fmt_secs(x0)),
        Span::raw(super::fmt_secs(mid)),
        Span::raw(super::fmt_secs(x1)),
    ]
}

fn fmt_axis(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(v.log10().floor());
    let n = v / mag;
    let step = if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * mag
}
