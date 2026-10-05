//! Round-trip latency / loss tracker.
//!
//! Loss is declared by timeout: a probe with no echo after `LossPolicy::timeout` is `Lost`. If the
//! echo later turns up it is reclassified `Late` (lost -1, late +1) so the counters stay honest
//! over very slow links. Jitter follows the RFC 3550 estimator.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;

use super::{ChartPoint, LatencySnapshot, RingBuffer, Sample, SampleStatus};

#[derive(Debug, Clone, Copy)]
pub struct LossPolicy {
    pub timeout: Duration,
}

impl LossPolicy {
    /// Generous default: a probe is not lost until well past any plausible RTT.
    pub fn for_interval(interval: Duration) -> Self {
        Self {
            timeout: (interval * 10).max(Duration::from_secs(2)),
        }
    }

    /// `for_interval` unless the config pins an explicit timeout (`ClientConfig::loss_timeout_ms`).
    pub fn resolve(interval: Duration, override_ms: u64) -> Self {
        if override_ms == 0 {
            Self::for_interval(interval)
        } else {
            Self {
                timeout: Duration::from_millis(override_ms),
            }
        }
    }
}

const SEEN_WINDOW: u64 = 4096;

pub struct LatencyTracker {
    started: Instant,
    policy: LossPolicy,
    hist: Histogram<u64>,
    inflight: BTreeMap<u64, Instant>,
    /// Sequence numbers already echoed, kept for the last `SEEN_WINDOW` seqs for dup detection.
    seen: VecDeque<u64>,
    highest_seq: Option<u64>,
    sent: u64,
    received: u64,
    lost: u64,
    failed: u64,
    late: u64,
    duplicates: u64,
    out_of_order: u64,
    min_us: u64,
    max_us: u64,
    sum_us: u128,
    jitter_us: f64,
    prev_rtt_us: Option<u64>,
    last_ms: Option<f64>,
    chart: RingBuffer<ChartPoint>,
}

impl LatencyTracker {
    pub fn new(policy: LossPolicy, chart_capacity: usize) -> Self {
        Self {
            started: Instant::now(),
            policy,
            // 1 µs .. 60 s at 3 significant figures: ~constant memory, fine resolution.
            hist: Histogram::new_with_bounds(1, 60_000_000, 3).expect("valid histogram bounds"),
            inflight: BTreeMap::new(),
            seen: VecDeque::with_capacity(SEEN_WINDOW as usize),
            highest_seq: None,
            sent: 0,
            received: 0,
            lost: 0,
            failed: 0,
            late: 0,
            duplicates: 0,
            out_of_order: 0,
            min_us: u64::MAX,
            max_us: 0,
            sum_us: 0,
            jitter_us: 0.0,
            prev_rtt_us: None,
            last_ms: None,
            chart: RingBuffer::new(chart_capacity),
        }
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn elapsed_s(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn on_sent(&mut self, seq: u64, at: Instant) {
        self.sent += 1;
        self.inflight.insert(seq, at);
    }

    /// Record an echo. Returns the classified sample for sinks / UI.
    pub fn on_echo(&mut self, seq: u64, rtt: Duration, now: Instant) -> Sample {
        let rtt_us = rtt.as_micros().max(1) as u64;
        let rtt_ms = rtt.as_secs_f64() * 1000.0;
        let elapsed_s = (now - self.started).as_secs_f64();
        let sent_at = chrono::Local::now() - chrono::Duration::from_std(rtt).unwrap_or_default();

        let status = if self.seen.contains(&seq) {
            self.duplicates += 1;
            SampleStatus::Duplicate
        } else {
            self.remember_seen(seq);
            let was_inflight = self.inflight.remove(&seq).is_some();
            let status = if !was_inflight {
                // Already timed out and counted as lost: give the count back.
                self.lost = self.lost.saturating_sub(1);
                self.late += 1;
                SampleStatus::Late
            } else if self.highest_seq.is_some_and(|h| seq < h) {
                self.out_of_order += 1;
                SampleStatus::OutOfOrder
            } else {
                SampleStatus::Ok
            };
            self.highest_seq = Some(self.highest_seq.map_or(seq, |h| h.max(seq)));
            self.received += 1;
            self.hist.saturating_record(rtt_us);
            self.min_us = self.min_us.min(rtt_us);
            self.max_us = self.max_us.max(rtt_us);
            self.sum_us += rtt_us as u128;
            if let Some(prev) = self.prev_rtt_us {
                let d = (rtt_us as f64 - prev as f64).abs();
                self.jitter_us += (d - self.jitter_us) / 16.0;
            }
            self.prev_rtt_us = Some(rtt_us);
            self.last_ms = Some(rtt_ms);
            self.chart.push(ChartPoint {
                t: elapsed_s,
                rtt_ms: Some(rtt_ms),
            });
            status
        };

        Sample {
            seq,
            sent_at,
            elapsed_s,
            rtt_ms: Some(rtt_ms),
            status,
            detail: None,
        }
    }

    /// A probe answered negatively before its timeout (connection refused, host unreachable).
    /// Counts as lost so loss % and thresholds behave; `failed` keeps the distinction.
    pub fn on_failed(&mut self, seq: u64, now: Instant, detail: impl Into<String>) -> Sample {
        let sent = self.inflight.remove(&seq);
        if sent.is_some() {
            self.lost += 1;
            self.failed += 1;
        }
        let sent_at = sent.unwrap_or(now);
        let elapsed_s = (sent_at - self.started).as_secs_f64();
        self.chart.push(ChartPoint {
            t: elapsed_s,
            rtt_ms: None,
        });
        Sample {
            seq,
            sent_at: chrono::Local::now()
                - chrono::Duration::from_std(now - sent_at).unwrap_or_default(),
            elapsed_s,
            rtt_ms: None,
            status: SampleStatus::Failed,
            detail: Some(detail.into()),
        }
    }

    /// Time out in-flight probes. Returns one `Lost` sample per expired probe.
    pub fn expire(&mut self, now: Instant) -> Vec<Sample> {
        let mut out = Vec::new();
        let timeout = self.policy.timeout;
        let expired: Vec<u64> = self
            .inflight
            .iter()
            .filter(|(_, sent)| now.duration_since(**sent) >= timeout)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in expired {
            let sent = self.inflight.remove(&seq).expect("just listed");
            self.lost += 1;
            let elapsed_s = (sent - self.started).as_secs_f64();
            self.chart.push(ChartPoint {
                t: elapsed_s,
                rtt_ms: None,
            });
            out.push(Sample {
                seq,
                sent_at: chrono::Local::now()
                    - chrono::Duration::from_std(now - sent).unwrap_or_default(),
                elapsed_s,
                rtt_ms: None,
                status: SampleStatus::Lost,
                detail: None,
            });
        }
        out
    }

    /// Declare everything still in flight lost (end of run).
    pub fn drain(&mut self) -> Vec<Sample> {
        let far_future = Instant::now() + self.policy.timeout * 2;
        self.expire(far_future)
    }

    pub fn chart_points(&self) -> Vec<ChartPoint> {
        self.chart.to_vec()
    }

    pub fn snapshot(&self) -> LatencySnapshot {
        let us = |v: u64| v as f64 / 1000.0;
        let has = self.received > 0;
        let completed = self.received + self.lost;
        LatencySnapshot {
            sent: self.sent,
            received: self.received,
            lost: self.lost,
            late: self.late,
            duplicates: self.duplicates,
            out_of_order: self.out_of_order,
            failed: self.failed,
            in_flight: self.inflight.len() as u64,
            loss_pct: if completed > 0 {
                self.lost as f64 * 100.0 / completed as f64
            } else {
                0.0
            },
            min_ms: if has { us(self.min_us) } else { 0.0 },
            avg_ms: if has {
                self.sum_us as f64 / self.received as f64 / 1000.0
            } else {
                0.0
            },
            max_ms: if has { us(self.max_us) } else { 0.0 },
            p50_ms: if has {
                us(self.hist.value_at_quantile(0.50))
            } else {
                0.0
            },
            p95_ms: if has {
                us(self.hist.value_at_quantile(0.95))
            } else {
                0.0
            },
            p99_ms: if has {
                us(self.hist.value_at_quantile(0.99))
            } else {
                0.0
            },
            jitter_ms: self.jitter_us / 1000.0,
            last_ms: self.last_ms,
            elapsed: self.started.elapsed(),
        }
    }

    fn remember_seen(&mut self, seq: u64) {
        if self.seen.len() as u64 >= SEEN_WINDOW {
            self.seen.pop_front();
        }
        self.seen.push_back(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> LatencyTracker {
        LatencyTracker::new(
            LossPolicy {
                timeout: Duration::from_millis(100),
            },
            64,
        )
    }

    #[test]
    fn ok_path_and_percentiles() {
        let mut t = tracker();
        let base = Instant::now();
        for i in 0..100u64 {
            t.on_sent(i, base);
            let s = t.on_echo(i, Duration::from_millis(i + 1), base);
            assert_eq!(s.status, SampleStatus::Ok);
        }
        let s = t.snapshot();
        assert_eq!(s.sent, 100);
        assert_eq!(s.received, 100);
        assert_eq!(s.lost, 0);
        assert!((s.min_ms - 1.0).abs() < 0.01);
        assert!((s.max_ms - 100.0).abs() < 0.01);
        assert!((s.avg_ms - 50.5).abs() < 0.01);
        assert!(s.p50_ms >= 49.0 && s.p50_ms <= 52.0, "p50 {}", s.p50_ms);
        assert!(s.p99_ms >= 98.0, "p99 {}", s.p99_ms);
        assert!(s.jitter_ms > 0.0);
    }

    #[test]
    fn loss_then_late_reclassifies() {
        let mut t = tracker();
        let base = Instant::now();
        t.on_sent(1, base);
        t.on_sent(2, base);
        let lost = t.expire(base + Duration::from_millis(150));
        assert_eq!(lost.len(), 2);
        assert_eq!(t.snapshot().lost, 2);
        assert!((t.snapshot().loss_pct - 100.0).abs() < 0.01);
        let s = t.on_echo(
            1,
            Duration::from_millis(200),
            base + Duration::from_millis(200),
        );
        assert_eq!(s.status, SampleStatus::Late);
        let snap = t.snapshot();
        assert_eq!(snap.lost, 1);
        assert_eq!(snap.late, 1);
        assert_eq!(snap.received, 1);
    }

    #[test]
    fn out_of_order_and_duplicates() {
        let mut t = tracker();
        let base = Instant::now();
        for i in 0..3 {
            t.on_sent(i, base);
        }
        assert_eq!(
            t.on_echo(2, Duration::from_millis(5), base).status,
            SampleStatus::Ok
        );
        assert_eq!(
            t.on_echo(1, Duration::from_millis(5), base).status,
            SampleStatus::OutOfOrder
        );
        assert_eq!(
            t.on_echo(1, Duration::from_millis(5), base).status,
            SampleStatus::Duplicate
        );
        let s = t.snapshot();
        assert_eq!(s.out_of_order, 1);
        assert_eq!(s.duplicates, 1);
        assert_eq!(s.received, 2);
        assert_eq!(s.in_flight, 1);
        assert_eq!(t.drain().len(), 1);
        assert_eq!(t.snapshot().lost, 1);
    }

    #[test]
    fn failed_counts_as_lost_once() {
        let mut t = tracker();
        let base = Instant::now();
        t.on_sent(1, base);
        t.on_sent(2, base);
        let s = t.on_failed(1, base + Duration::from_millis(1), "connection refused");
        assert_eq!(s.status, SampleStatus::Failed);
        assert_eq!(s.detail.as_deref(), Some("connection refused"));
        let snap = t.snapshot();
        assert_eq!((snap.lost, snap.failed, snap.in_flight), (1, 1, 1));
        // Already-expired probe reported as failed later: no double counting.
        t.expire(base + Duration::from_millis(500));
        t.on_failed(2, base + Duration::from_secs(1), "late failure");
        let snap = t.snapshot();
        assert_eq!((snap.lost, snap.failed), (2, 1));
        assert!(t.chart_points().iter().all(|p| p.rtt_ms.is_none()));
    }
}
