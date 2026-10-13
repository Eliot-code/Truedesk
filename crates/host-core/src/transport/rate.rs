//! Video bitrate adaptation shared between a transport and the encoder.
//!
//! The encoder publishes the bitrate its stream size calls for (the
//! ceiling); the transport's estimator lowers the target below it when the
//! path shows loss or queueing delay and raises it again once the path is
//! clean. The encoder applies the target on its next frame.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The estimator never asks for less than this.
pub const MIN_TARGET_BITRATE: u32 = 1_500_000;
/// Datagrams are paced this much faster than the target bitrate, so a
/// frame of average size leaves well within its frame interval.
const PACING_FACTOR_PERCENT: u64 = 150;
/// Pacing never drops below this rate, so keyframes still leave quickly at
/// the bitrate floor.
const MIN_PACING_BITRATE: u64 = 3_000_000;

/// Bitrate state shared by the transport (writer of the target) and the
/// encoder (writer of the ceiling, reader of the target).
#[derive(Debug, Default)]
pub struct RateControl {
    ceiling: AtomicU32,
    /// 0 until an estimator has an opinion: the encoder then uses the
    /// ceiling, which is the pre-adaptation behaviour.
    target: AtomicU32,
}

impl RateControl {
    /// The bitrate the current stream size calls for.
    pub fn set_ceiling(&self, bps: u32) { self.ceiling.store(bps, Ordering::Relaxed); }

    pub fn ceiling(&self) -> u32 { self.ceiling.load(Ordering::Relaxed) }

    /// The bitrate the encoder should use now; 0 while no ceiling is known.
    pub fn target(&self) -> u32 {
        let ceiling = self.ceiling();
        match self.target.load(Ordering::Relaxed) {
            0 => ceiling,
            target => target.min(ceiling),
        }
    }

    pub(crate) fn set_target(&self, bps: u32) { self.target.store(bps, Ordering::Relaxed); }

    /// The rate datagrams are paced at (bits per second); `None` while the
    /// stream bitrate is unknown.
    pub(crate) fn pacing_rate(&self) -> Option<u64> {
        let target = u64::from(self.target());
        (target > 0).then(|| (target * PACING_FACTOR_PERCENT / 100).max(MIN_PACING_BITRATE))
    }
}

/// What a receiver report says about the path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathSample {
    pub rtt: Option<Duration>,
    /// Fraction of datagrams lost on first transmission, 0..=1.
    pub loss: f64,
}

// Loss thresholds follow WebRTC's GCC: a few percent of random (Wi-Fi) loss
// is repaired by retransmission and is no reason to lower quality.
/// Loss above this cuts the bitrate hard.
const HEAVY_LOSS: f64 = 0.25;
/// Loss above this cuts the bitrate gently.
const LOSS: f64 = 0.10;
/// Loss below this (and low delay) lets the bitrate grow.
const CLEAN_LOSS: f64 = 0.02;
/// Queueing delay (RTT above the recent minimum) thresholds.
const HEAVY_QUEUE_DELAY: Duration = Duration::from_millis(150);
const QUEUE_DELAY: Duration = Duration::from_millis(50);
const CLEAN_QUEUE_DELAY: Duration = Duration::from_millis(25);
const HEAVY_DECREASE: f64 = 0.7;
const DECREASE: f64 = 0.85;
/// Decreases closer together than this count once, so one congestion
/// episode reported several times does not compound.
const DECREASE_SPACING: Duration = Duration::from_millis(300);
/// After a decrease the bitrate holds this long before growing again.
const HOLD_AFTER_DECREASE: Duration = Duration::from_millis(1_000);
/// Additive increase, as a fraction of the ceiling per second.
const INCREASE_PER_SEC: f64 = 0.08;
/// The minimum RTT is tracked over this window.
const MIN_RTT_WINDOW: Duration = Duration::from_secs(10);
/// A peer that reported before and then goes quiet this long is treated
/// as heavy loss (its reports, or our packets, are not getting through).
const SILENCE: Duration = Duration::from_millis(1_000);
const LOSS_SMOOTHING: f64 = 0.5;

/// Additive-increase / multiplicative-decrease bitrate estimator driven by
/// loss and queueing delay (RTT growth over its recent minimum).
#[derive(Debug, Default)]
pub(crate) struct AimdEstimator {
    /// Bits per second; 0 until the first update (then starts at the
    /// ceiling, which is the pre-adaptation behaviour).
    target: f64,
    loss: f64,
    srtt: Option<Duration>,
    rtts: VecDeque<(Instant, Duration)>,
    last_update: Option<Instant>,
    last_decrease: Option<Instant>,
    last_report: Option<Instant>,
}

impl AimdEstimator {
    pub(crate) fn srtt(&self) -> Option<Duration> { self.srtt }

    /// Feeds one receiver report; returns the new target (bps).
    pub(crate) fn on_report(&mut self, now: Instant, ceiling: u32, sample: PathSample) -> u32 {
        self.last_report = Some(now);
        self.loss = if self.last_update.is_none() { sample.loss } else { self.loss + LOSS_SMOOTHING * (sample.loss - self.loss) };
        if let Some(rtt) = sample.rtt {
            self.srtt = Some(self.srtt.map_or(rtt, |srtt| srtt.mul_f64(0.75) + rtt.mul_f64(0.25)));
            while self.rtts.front().is_some_and(|&(at, _)| now.duration_since(at) > MIN_RTT_WINDOW) {
                self.rtts.pop_front();
            }
            self.rtts.push_back((now, rtt));
        }
        let queue_delay = match (self.srtt, self.rtts.iter().map(|&(_, rtt)| rtt).min()) {
            (Some(srtt), Some(min)) => srtt.saturating_sub(min),
            _ => Duration::ZERO,
        };
        let signal = if self.loss >= HEAVY_LOSS || queue_delay >= HEAVY_QUEUE_DELAY {
            Signal::Decrease(HEAVY_DECREASE)
        } else if self.loss >= LOSS || queue_delay >= QUEUE_DELAY {
            Signal::Decrease(DECREASE)
        } else if self.loss < CLEAN_LOSS && queue_delay < CLEAN_QUEUE_DELAY {
            Signal::Increase
        } else {
            Signal::Hold
        };
        self.apply(now, ceiling, signal)
    }

    /// Relay-path input: whether the send queue backed up since the last
    /// sample. Returns the new target (bps).
    pub(crate) fn on_queue_sample(&mut self, now: Instant, ceiling: u32, congested: bool) -> u32 {
        let signal = if congested { Signal::Decrease(DECREASE) } else { Signal::Increase };
        self.apply(now, ceiling, signal)
    }

    /// Periodic check for a peer that stopped reporting. Returns the new
    /// target, or `None` when nothing changed.
    pub(crate) fn on_tick(&mut self, now: Instant, ceiling: u32) -> Option<u32> {
        let last = self.last_report?;
        if now.duration_since(last) < SILENCE {
            return None;
        }
        // One decrease per silent second.
        self.last_report = Some(now);
        Some(self.apply(now, ceiling, Signal::Decrease(HEAVY_DECREASE)))
    }

    fn apply(&mut self, now: Instant, ceiling: u32, signal: Signal) -> u32 {
        let ceiling = f64::from(ceiling.max(1));
        let floor = f64::from(MIN_TARGET_BITRATE).min(ceiling);
        if self.target == 0.0 {
            self.target = ceiling;
        }
        let elapsed = self.last_update.map_or(Duration::ZERO, |at| now.saturating_duration_since(at)).min(Duration::from_millis(500));
        self.last_update = Some(now);
        match signal {
            Signal::Decrease(factor) => {
                if self.last_decrease.is_none_or(|at| now.saturating_duration_since(at) >= DECREASE_SPACING) {
                    self.target *= factor;
                    self.last_decrease = Some(now);
                }
            }
            Signal::Increase => {
                if self.last_decrease.is_none_or(|at| now.saturating_duration_since(at) >= HOLD_AFTER_DECREASE) {
                    self.target += ceiling * INCREASE_PER_SEC * elapsed.as_secs_f64();
                }
            }
            Signal::Hold => {}
        }
        self.target = self.target.clamp(floor, ceiling);
        self.target as u32
    }
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Decrease(f64),
    Increase,
    Hold,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEILING: u32 = 12_000_000;
    const STEP: Duration = Duration::from_millis(100);

    fn clean(rtt_ms: u64) -> PathSample { PathSample { rtt: Some(Duration::from_millis(rtt_ms)), loss: 0.0 } }

    #[test]
    fn rate_control_defaults_to_the_ceiling_and_clamps_the_target() {
        let rate = RateControl::default();
        assert_eq!(rate.target(), 0);
        assert_eq!(rate.pacing_rate(), None);
        rate.set_ceiling(CEILING);
        assert_eq!(rate.target(), CEILING);
        assert_eq!(rate.pacing_rate(), Some(18_000_000));
        rate.set_target(4_000_000);
        assert_eq!(rate.target(), 4_000_000);
        rate.set_ceiling(2_000_000);
        assert_eq!(rate.target(), 2_000_000);
        assert_eq!(rate.pacing_rate(), Some(MIN_PACING_BITRATE));
    }

    #[test]
    fn clean_path_stays_at_the_ceiling() {
        let mut estimator = AimdEstimator::default();
        let mut now = Instant::now();
        for _ in 0..50 {
            assert_eq!(estimator.on_report(now, CEILING, clean(20)), CEILING);
            now += STEP;
        }
    }

    #[test]
    fn light_random_loss_keeps_the_ceiling() {
        // 5% loss on a steady RTT is repaired by NACKs, not a congestion signal.
        let mut estimator = AimdEstimator::default();
        let mut now = Instant::now();
        let lossy = PathSample { rtt: Some(Duration::from_millis(20)), loss: 0.05 };
        for _ in 0..50 {
            assert_eq!(estimator.on_report(now, CEILING, lossy), CEILING);
            now += STEP;
        }
    }

    #[test]
    fn heavy_loss_cuts_multiplicatively_once_per_spacing() {
        let mut estimator = AimdEstimator::default();
        let now = Instant::now();
        let lossy = PathSample { rtt: Some(Duration::from_millis(20)), loss: 0.3 };
        assert_eq!(estimator.on_report(now, CEILING, lossy), (f64::from(CEILING) * HEAVY_DECREASE) as u32);
        // A second report inside the spacing does not compound.
        assert_eq!(estimator.on_report(now + STEP, CEILING, lossy), (f64::from(CEILING) * HEAVY_DECREASE) as u32);
        let after = estimator.on_report(now + DECREASE_SPACING, CEILING, lossy);
        assert_eq!(after, (f64::from(CEILING) * HEAVY_DECREASE * HEAVY_DECREASE) as u32);
    }

    #[test]
    fn sustained_loss_bottoms_out_at_the_floor() {
        let mut estimator = AimdEstimator::default();
        let mut now = Instant::now();
        let mut target = CEILING;
        for _ in 0..100 {
            target = estimator.on_report(now, CEILING, PathSample { rtt: None, loss: 0.5 });
            now += STEP;
        }
        assert_eq!(target, MIN_TARGET_BITRATE);
    }

    #[test]
    fn floor_never_exceeds_a_small_ceiling() {
        let mut estimator = AimdEstimator::default();
        let target = estimator.on_report(Instant::now(), 1_000_000, PathSample { rtt: None, loss: 0.5 });
        assert_eq!(target, 1_000_000);
    }

    #[test]
    fn rising_rtt_reads_as_congestion() {
        let mut estimator = AimdEstimator::default();
        let mut now = Instant::now();
        for _ in 0..10 {
            estimator.on_report(now, CEILING, clean(20));
            now += STEP;
        }
        let mut target = CEILING;
        for _ in 0..10 {
            target = estimator.on_report(now, CEILING, clean(300));
            now += STEP;
        }
        assert!(target < CEILING / 2, "target {target}");
    }

    #[test]
    fn recovers_additively_after_the_hold() {
        let mut estimator = AimdEstimator::default();
        let start = Instant::now();
        let mut target = estimator.on_report(start, CEILING, PathSample { rtt: None, loss: 0.5 });
        // While the smoothed loss clears and for the hold after the last
        // decrease, the bitrate does not grow.
        let mut now = start;
        while now + STEP < start + HOLD_AFTER_DECREASE {
            now += STEP;
            let next = estimator.on_report(now, CEILING, clean(20));
            assert!(next <= target, "grew during the hold: {next} > {target}");
            target = next;
        }
        while now < start + Duration::from_secs(3) {
            now += STEP;
            target = estimator.on_report(now, CEILING, clean(20));
        }
        // From then on each second of clean reports adds ~8% of the ceiling.
        let before = target;
        for _ in 0..10 {
            now += STEP;
            target = estimator.on_report(now, CEILING, clean(20));
        }
        let gained = f64::from(target - before);
        assert!((gained - f64::from(CEILING) * INCREASE_PER_SEC).abs() < 1_000.0, "gained {gained}");
    }

    #[test]
    fn silence_after_reports_decreases_but_never_reporting_does_not() {
        let mut estimator = AimdEstimator::default();
        let start = Instant::now();
        assert_eq!(estimator.on_tick(start + Duration::from_secs(5), CEILING), None);
        estimator.on_report(start, CEILING, clean(20));
        assert_eq!(estimator.on_tick(start + Duration::from_millis(500), CEILING), None);
        let cut = estimator.on_tick(start + SILENCE, CEILING).unwrap();
        assert_eq!(cut, (f64::from(CEILING) * HEAVY_DECREASE) as u32);
        assert_eq!(estimator.on_tick(start + SILENCE + Duration::from_millis(10), CEILING), None);
    }

    #[test]
    fn queue_samples_drive_the_relay_estimate() {
        let mut estimator = AimdEstimator::default();
        let now = Instant::now();
        assert_eq!(estimator.on_queue_sample(now, CEILING, false), CEILING);
        assert_eq!(estimator.on_queue_sample(now, CEILING, true), (f64::from(CEILING) * DECREASE) as u32);
    }
}
