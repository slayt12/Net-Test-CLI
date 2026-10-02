//! RTT-over-time SVG: polyline for RTT, dots on the baseline for losses, light grid and ticks.
//! Points are downsampled to at most one per output pixel so huge runs stay small.

use crate::stats::ChartPoint;

const PAD_L: f64 = 48.0;
const PAD_R: f64 = 12.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 28.0;

pub fn rtt_chart(points: &[ChartPoint], w: f64, h: f64) -> String {
    let plot_w = w - PAD_L - PAD_R;
    let plot_h = h - PAD_T - PAD_B;
    let t_max = points.iter().map(|p| p.t).fold(0.0_f64, f64::max).max(1e-9);
    let rtt_max = points
        .iter()
        .filter_map(|p| p.rtt_ms)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let y_max = nice_ceiling(rtt_max * 1.1);

    let x = |t: f64| PAD_L + t / t_max * plot_w;
    let y = |v: f64| PAD_T + plot_h - (v / y_max).min(1.0) * plot_h;

    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"RTT over time\">"
    );

    // Grid + Y ticks.
    for i in 0..=4 {
        let v = y_max * i as f64 / 4.0;
        let yy = y(v);
        s.push_str(&format!(
            "<line class=\"grid\" x1=\"{PAD_L}\" x2=\"{:.1}\" y1=\"{yy:.1}\" y2=\"{yy:.1}\"/>",
            w - PAD_R
        ));
        s.push_str(&format!(
            "<text class=\"tick\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
            PAD_L - 6.0,
            yy + 4.0,
            fmt_num(v)
        ));
    }
    // X ticks.
    for i in 0..=6 {
        let t = t_max * i as f64 / 6.0;
        let xx = x(t);
        s.push_str(&format!(
            "<text class=\"tick\" x=\"{xx:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
            h - 8.0,
            fmt_time(t)
        ));
    }
    s.push_str(&format!(
        "<line class=\"axis\" x1=\"{PAD_L}\" x2=\"{:.1}\" y1=\"{:.1}\" y2=\"{:.1}\"/>",
        w - PAD_R,
        PAD_T + plot_h,
        PAD_T + plot_h
    ));

    // Downsample: keep max RTT per pixel column so spikes survive.
    let cols = plot_w.max(1.0) as usize;
    let mut col_max: Vec<Option<(f64, f64)>> = vec![None; cols + 1];
    let mut losses: Vec<f64> = Vec::new();
    for p in points {
        match p.rtt_ms {
            Some(v) => {
                let c = ((p.t / t_max) * cols as f64)
                    .round()
                    .clamp(0.0, cols as f64) as usize;
                let slot = &mut col_max[c];
                if slot.is_none_or(|(_, m)| v > m) {
                    *slot = Some((p.t, v));
                }
            }
            None => losses.push(p.t),
        }
    }
    let mut poly = String::new();
    for (t, v) in col_max.iter().flatten() {
        poly.push_str(&format!("{:.1},{:.1} ", x(*t), y(*v)));
    }
    if !poly.is_empty() {
        s.push_str(&format!(
            "<polyline class=\"rtt-line\" points=\"{}\"/>",
            poly.trim_end()
        ));
    }
    for t in losses.iter().take(5000) {
        s.push_str(&format!(
            "<circle class=\"loss-dot\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"3\"/>",
            x(*t),
            PAD_T + plot_h - 3.0
        ));
    }
    s.push_str("</svg>");
    s
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

fn fmt_num(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn fmt_time(t: f64) -> String {
    if t >= 3600.0 {
        format!(
            "{}h{:02}m",
            (t / 3600.0) as u64,
            ((t % 3600.0) / 60.0) as u64
        )
    } else if t >= 60.0 {
        format!("{}m{:02}s", (t / 60.0) as u64, (t % 60.0) as u64)
    } else {
        format!("{t:.0}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_has_polyline_and_loss_dots() {
        let pts: Vec<ChartPoint> = (0..50)
            .map(|i| ChartPoint {
                t: i as f64,
                rtt_ms: if i % 10 == 0 {
                    None
                } else {
                    Some(10.0 + i as f64)
                },
            })
            .collect();
        let svg = rtt_chart(&pts, 900.0, 300.0);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<polyline"));
        assert_eq!(svg.matches("<circle").count(), 5);
        assert_eq!(nice_ceiling(73.0), 100.0);
        assert_eq!(nice_ceiling(1.3), 2.0);
    }
}
