// crates/audiolib-remote/src/bandwidth.rs
//! Moving estimate of the bandwidth available from the server, measured from recent
//! transfers (audio streams, previews, snapshots). Audition picks its path from it
//! ([`crate::choose_route`]); before the first measurement it assumes a conservative
//! value, even for a LAN address: a VPN or a slow Wi-Fi link looks like a LAN too.
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Samples shorter than this are dominated by latency and TCP slow start.
const MIN_SAMPLE_BYTES: u64 = 256 * 1024;
const MIN_SAMPLE_TIME: Duration = Duration::from_millis(100);
/// Weight of a new sample in the moving average.
const ALPHA: f64 = 0.35;

/// Exponentially weighted throughput, in bits per second.
#[derive(Debug, Default)]
pub struct Bandwidth {
    estimate: Mutex<Option<f64>>,
}

static GLOBAL: Bandwidth = Bandwidth::new();

/// The process-wide estimate shared by all server transfers.
pub fn bandwidth() -> &'static Bandwidth {
    &GLOBAL
}

impl Bandwidth {
    pub const fn new() -> Self {
        Self {
            estimate: Mutex::new(None),
        }
    }

    /// Record `bytes` received while actively reading for `elapsed` (time spent waiting
    /// for the player to catch up must not be included). Tiny samples are ignored.
    pub fn record(&self, bytes: u64, elapsed: Duration) {
        if bytes < MIN_SAMPLE_BYTES || elapsed < MIN_SAMPLE_TIME {
            return;
        }
        let sample = bytes as f64 * 8.0 / elapsed.as_secs_f64();
        let mut estimate = self.estimate.lock().unwrap_or_else(PoisonError::into_inner);
        *estimate = Some(match *estimate {
            Some(previous) => previous + ALPHA * (sample - previous),
            None => sample,
        });
    }

    /// Current estimate in bits per second; `None` before the first usable sample.
    pub fn estimate_bps(&self) -> Option<f64> {
        *self.estimate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forget the estimate (another server was configured).
    pub fn reset(&self) {
        *self.estimate.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// Measures the active transfer time of one response in segments, so pauses (the player's
/// window is full) do not count as slow network.
#[derive(Debug)]
pub(crate) struct Throughput {
    started: Option<std::time::Instant>,
    bytes: u64,
}

/// A segment of at least [`MIN_SAMPLE_BYTES`] that lasted this long is recorded and a new
/// one started, so the estimate follows a long transfer (on a slow link a segment simply
/// lasts until it holds enough bytes).
const SEGMENT: Duration = Duration::from_millis(500);

impl Throughput {
    pub(crate) fn new() -> Self {
        Self {
            started: None,
            bytes: 0,
        }
    }

    /// Bytes arrived (starts a segment on the first call after [`Self::pause`]).
    pub(crate) fn add(&mut self, bytes: u64, into: &Bandwidth) {
        let now = std::time::Instant::now();
        let started = *self.started.get_or_insert(now);
        self.bytes += bytes;
        if self.bytes >= MIN_SAMPLE_BYTES && now.duration_since(started) >= SEGMENT {
            into.record(self.bytes, now.duration_since(started));
            self.started = Some(now);
            self.bytes = 0;
        }
    }

    /// The transfer stops for a while (or ends): record the open segment.
    pub(crate) fn pause(&mut self, into: &Bandwidth) {
        if let Some(started) = self.started.take() {
            into.record(self.bytes, started.elapsed());
        }
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_moves_towards_new_samples_and_ignores_tiny_ones() {
        let bandwidth = Bandwidth::new();
        assert_eq!(bandwidth.estimate_bps(), None);
        bandwidth.record(1000, Duration::from_secs(1));
        assert_eq!(bandwidth.estimate_bps(), None, "too small to measure");
        // 1.25 MB in 1 s = 10 Mbit/s.
        bandwidth.record(1_250_000, Duration::from_secs(1));
        assert_eq!(bandwidth.estimate_bps(), Some(10_000_000.0));
        bandwidth.record(625_000, Duration::from_secs(1));
        let estimate = bandwidth.estimate_bps().unwrap();
        assert!(estimate < 10_000_000.0 && estimate > 5_000_000.0);
        bandwidth.reset();
        assert_eq!(bandwidth.estimate_bps(), None);
    }

    #[test]
    fn slow_transfers_are_sampled_once_a_segment_holds_enough_bytes() {
        let bandwidth = Bandwidth::new();
        let mut throughput = Throughput::new();
        // 64 KiB every 0.3 s (about 1.7 Mbit/s): no sample until 256 KiB have arrived.
        throughput.add(64 * 1024, &bandwidth);
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(bandwidth.estimate_bps(), None);
            throughput.add(64 * 1024, &bandwidth);
        }
        let estimate = bandwidth.estimate_bps().expect("sampled at 256 KiB");
        assert!((1.0e6..2.5e6).contains(&estimate), "{estimate}");
    }
}
