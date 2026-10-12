//! Token-bucket pacing for the direct UDP path.
//!
//! Sending a large frame as one burst of datagrams overflows shallow Wi-Fi
//! and router queues, which then drop the tail of the frame. The pacer
//! spreads datagrams at a fixed rate instead, with a small burst allowance so
//! coarse timers do not starve the link.

use std::time::{Duration, Instant};

/// Datagrams always fit in the bucket, even at very low rates.
const MIN_BURST_BYTES: f64 = 8.0 * 1_200.0;
/// The bucket holds this much sending time at the current rate.
const BURST_TIME: f64 = 0.004;

#[derive(Debug)]
pub(crate) struct Pacer {
    /// Bytes per second; `None` sends without pacing.
    rate: Option<f64>,
    tokens: f64,
    capacity: f64,
    last: Instant,
}

impl Pacer {
    /// An unpaced pacer (no rate yet).
    pub(crate) fn new(now: Instant) -> Self {
        Self { rate: None, tokens: MIN_BURST_BYTES, capacity: MIN_BURST_BYTES, last: now }
    }

    /// Sets the rate in bits per second; `None` or zero disables pacing.
    pub(crate) fn set_rate(&mut self, now: Instant, bits_per_sec: Option<u64>) {
        let rate = bits_per_sec.filter(|&bps| bps > 0).map(|bps| bps as f64 / 8.0);
        if rate == self.rate {
            return;
        }
        self.refill(now);
        self.rate = rate;
        self.capacity = rate.map_or(MIN_BURST_BYTES, |rate| (rate * BURST_TIME).max(MIN_BURST_BYTES));
        self.tokens = self.tokens.min(self.capacity);
    }

    /// Takes `len` bytes of budget if available now; otherwise returns how
    /// long until it will be.
    pub(crate) fn try_take(&mut self, now: Instant, len: usize) -> Result<(), Duration> {
        let Some(rate) = self.rate else { return Ok(()) };
        self.refill(now);
        let len = len as f64;
        if self.tokens >= len.min(self.capacity) {
            self.tokens -= len;
            return Ok(());
        }
        Err(Duration::from_secs_f64((len.min(self.capacity) - self.tokens) / rate))
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        if let Some(rate) = self.rate {
            self.tokens = (self.tokens + elapsed * rate).min(self.capacity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends `count` datagrams of `len` bytes as fast as the pacer allows,
    /// on a simulated clock, and returns when the last one went out.
    fn drain(pacer: &mut Pacer, start: Instant, count: usize, len: usize) -> Duration {
        let mut now = start;
        for _ in 0..count {
            loop {
                match pacer.try_take(now, len) {
                    Ok(()) => break,
                    Err(wait) => {
                        assert!(wait > Duration::ZERO);
                        now += wait;
                    }
                }
            }
        }
        now - start
    }

    #[test]
    fn unpaced_sends_everything_at_once() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        assert_eq!(drain(&mut pacer, start, 1_000, 1_108), Duration::ZERO);
    }

    #[test]
    fn spreads_a_frame_at_the_configured_rate() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        // 8 Mbps = 1 MB/s; the burst allowance is 8 * 1200 bytes.
        pacer.set_rate(start, Some(8_000_000));
        let took = drain(&mut pacer, start, 100, 1_000);
        let expected = (100_000.0 - MIN_BURST_BYTES) / 1_000_000.0;
        assert!((took.as_secs_f64() - expected).abs() < 0.001, "took {took:?}, expected {expected}s");
    }

    #[test]
    fn burst_allowance_scales_with_rate() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        // 80 Mbps = 10 MB/s: 4 ms of burst is 40 kB.
        pacer.set_rate(start, Some(80_000_000));
        let now = start + Duration::from_secs(1);
        let mut sent = 0;
        while pacer.try_take(now, 1_000).is_ok() {
            sent += 1;
        }
        assert_eq!(sent, 40);
    }

    #[test]
    fn idle_time_does_not_bank_more_than_the_burst() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        pacer.set_rate(start, Some(8_000_000));
        let later = start + Duration::from_secs(10);
        let took = drain(&mut pacer, later, 20, 1_000);
        let expected = (20_000.0 - MIN_BURST_BYTES) / 1_000_000.0;
        assert!((took.as_secs_f64() - expected).abs() < 0.001, "took {took:?}");
    }

    #[test]
    fn disabling_the_rate_stops_pacing() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        pacer.set_rate(start, Some(1_000_000));
        assert!(drain(&mut pacer, start, 50, 1_000) > Duration::ZERO);
        pacer.set_rate(start, None);
        assert_eq!(drain(&mut pacer, start, 50, 1_000), Duration::ZERO);
    }
}
