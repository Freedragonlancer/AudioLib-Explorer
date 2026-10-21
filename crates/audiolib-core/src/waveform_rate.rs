// crates/audiolib-core/src/waveform_rate.rs
//! Client-side generation rate and ETA for the server Waveform cache (doc/api/README.md
//! §9.5), estimated from successive `GET /waveforms/status` readings instead of the
//! server's `current_rate` / `eta_secs` (which can lag far behind the real throughput).
//!
//! Only time spent actively generating counts: an interval is used when both of its
//! readings were `running` (not paused, yielding or idle) and close enough together.
//! A drop in the cached count (server restart, rebuild, cache eviction) starts over.
//! Pure and allocation-bounded; called from the UI thread on each poll, never from
//! audio code.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Active generation time the rate is averaged over.
pub const RATE_WINDOW: Duration = Duration::from_secs(180);
/// Active time needed before a rate is reported (「估算中」 until then).
pub const RATE_MIN_SPAN: Duration = Duration::from_secs(30);
/// Intervals longer than this (missed polls, sleep) are not trusted.
pub const RATE_MAX_GAP: Duration = Duration::from_secs(60);
/// Upper bound on kept points (a poll every 2 s fills the window with 90).
const MAX_POINTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    /// Accumulated active seconds.
    active: f64,
    /// Accumulated records generated while active.
    done: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    at: Instant,
    cached: u64,
    active: bool,
}

/// Rolling-window rate estimator over status readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveformRateEstimator {
    last: Option<Reading>,
    active: f64,
    done: u64,
    points: VecDeque<Point>,
}

impl WaveformRateEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every reading.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Record one reading: `cached` records at `at`; `active` when the server reports it
    /// is generating right now (running, not paused or yielding).
    pub fn observe(&mut self, at: Instant, cached: u64, active: bool) {
        let reading = Reading { at, cached, active };
        let Some(last) = self.last.replace(reading) else {
            self.anchor();
            return;
        };
        if cached < last.cached || at < last.at {
            // Restart, rebuild or eviction: earlier counts no longer compare.
            self.reset();
            self.last = Some(reading);
            self.anchor();
            return;
        }
        let gap = at - last.at;
        if !(active && last.active) || gap > RATE_MAX_GAP {
            return;
        }
        self.active += gap.as_secs_f64();
        self.done += cached - last.cached;
        if self.points.len() == MAX_POINTS {
            self.points.pop_front();
        }
        self.points.push_back(Point {
            active: self.active,
            done: self.done,
        });
        // Keep one point at or before the window start as the baseline.
        let start = self.active - RATE_WINDOW.as_secs_f64();
        while self.points.len() > 1 && self.points[1].active <= start {
            self.points.pop_front();
        }
    }

    fn anchor(&mut self) {
        if self.points.is_empty() {
            self.points.push_back(Point {
                active: self.active,
                done: self.done,
            });
        }
    }

    /// Records per second over the window, `None` until [`RATE_MIN_SPAN`] of active
    /// generation has been seen.
    pub fn rate(&self) -> Option<f64> {
        let (first, last) = (self.points.front()?, self.points.back()?);
        let span = last.active - first.active;
        (span >= RATE_MIN_SPAN.as_secs_f64()).then(|| (last.done - first.done) as f64 / span)
    }

    /// Seconds left for `remaining` records at [`Self::rate`]; `None` while estimating
    /// or when nothing was generated in the window.
    pub fn eta_secs(&self, remaining: u64) -> Option<u64> {
        if remaining == 0 {
            return Some(0);
        }
        let rate = self.rate().filter(|rate| *rate > 0.0)?;
        Some((remaining as f64 / rate).ceil() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(estimator: &mut WaveformRateEstimator, t0: Instant, readings: &[(u64, u64, bool)]) {
        for &(secs, cached, active) in readings {
            estimator.observe(t0 + Duration::from_secs(secs), cached, active);
        }
    }

    #[test]
    fn estimating_until_enough_active_time() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        assert_eq!(e.rate(), None);
        feed(
            &mut e,
            t0,
            &[(0, 100, true), (5, 105, true), (10, 110, true)],
        );
        assert_eq!(e.rate(), None);
        assert_eq!(e.eta_secs(50), None);
        feed(&mut e, t0, &[(20, 120, true), (30, 130, true)]);
        assert!((e.rate().unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(e.eta_secs(3600), Some(3600));
        assert_eq!(e.eta_secs(0), Some(0));
    }

    #[test]
    fn window_follows_recent_rate() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        let mut cached = 0;
        // 10 minutes at 0.2/s, then 4 minutes at 1/s.
        for i in 0..=120 {
            e.observe(t0 + Duration::from_secs(i * 5), cached, true);
            cached += 1;
        }
        for i in 121..=168 {
            cached += 5;
            e.observe(t0 + Duration::from_secs(i * 5), cached, true);
        }
        let rate = e.rate().unwrap();
        assert!((rate - 1.0).abs() < 0.05, "{rate}");
        assert!(e.points.len() <= 40);
    }

    #[test]
    fn paused_and_yielding_time_is_ignored() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        feed(
            &mut e,
            t0,
            &[
                (0, 0, true),
                (20, 20, true),
                (25, 20, false),
                (600, 20, false),
                (605, 25, true),
                (625, 45, true),
            ],
        );
        // 40 active seconds, 40 records; the 10-minute pause does not dilute it.
        assert!((e.rate().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn long_gaps_are_skipped() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        feed(
            &mut e,
            t0,
            &[
                (0, 0, true),
                (20, 20, true),
                (500, 30, true),
                (520, 50, true),
            ],
        );
        assert!((e.rate().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn count_decrease_starts_over() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        feed(&mut e, t0, &[(0, 500, true), (40, 540, true)]);
        assert!(e.rate().is_some());
        feed(&mut e, t0, &[(45, 10, true), (50, 15, true)]);
        assert_eq!(e.rate(), None, "restart: estimating again");
        feed(&mut e, t0, &[(80, 45, true)]);
        assert!((e.rate().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn stalled_generation_has_no_eta() {
        let t0 = Instant::now();
        let mut e = WaveformRateEstimator::new();
        feed(&mut e, t0, &[(0, 7, true), (30, 7, true), (60, 7, true)]);
        assert_eq!(e.rate(), Some(0.0));
        assert_eq!(e.eta_secs(10), None);
    }
}
