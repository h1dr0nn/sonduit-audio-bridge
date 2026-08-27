//! Clock drift estimation between the sender's and receiver's sample clocks.
//!
//! Two devices nominally running at 48 kHz do not agree. Consumer crystals are
//! specified in tens of parts per million, so a 20 ppm disagreement moves
//! roughly `48000 * 20e-6 = 0.96` frames per second, about 3456 frames per
//! hour. Left alone that either drains the jitter buffer to a permanent
//! underrun or grows it until latency is unacceptable.
//!
//! Drift is a slope, jitter is noise around it. This module estimates the
//! slope by least squares over a window of observations, which averages the
//! noise out; [`crate::jitter`] handles the noise itself and must not try to
//! correct drift.
//!
//! Nothing here reads a clock: observations are supplied by the caller.

use std::collections::VecDeque;

/// Observations retained for the regression.
///
/// # Why this is large
///
/// Drift is a slope and jitter is noise about it, so the window has to be long
/// enough for the slope to accumulate past the noise. The arithmetic decides
/// the number, and it is unforgiving:
///
/// over a window of `T` seconds, drift of `D` ppm displaces arrival times by
/// `D * T` seconds in total, while jitter of amplitude `J` leaves a residual
/// slope uncertainty on the order of `J / sqrt(N)`. Resolving the drift needs
///
/// ```text
/// D * T  >>  J / sqrt(N)
/// ```
///
/// At 6 ms packets with 3 ms of jitter, a 512 observation window spans 3.1 s.
/// A 40 ppm drift displaces arrivals by only 40e-6 * 3.1 = 124 us over that
/// span, while the noise term is 3 ms / sqrt(512) = 133 us. The signal is
/// smaller than the noise and the estimate is worthless. This was not a
/// theoretical concern: a test caught it.
///
/// 4096 observations spans about 25 s, giving 983 us of signal against 47 us
/// of noise, a ratio of roughly 20. That is the smallest window that actually
/// works.
///
/// A long window is the right answer anyway. Crystal drift is a physical
/// constant of the two devices; it does not change from second to second, so
/// there is nothing to be gained by tracking it quickly.
const WINDOW: usize = 4096;

/// Observations required before an estimate is offered at all.
///
/// About 3 s of audio. Enough to be better than nothing for a large drift,
/// while [`DriftEstimator::resolution_ppm`] reports what is actually
/// resolvable so a caller never trusts an estimate more than it deserves.
const MIN_OBSERVATIONS: usize = 512;

/// One paired reading of the two clocks.
#[derive(Debug, Clone, Copy)]
struct Observation {
    /// Frames the sender says it has produced.
    sender_frames: f64,
    /// Receiver-side monotonic time at which that was observed, in seconds.
    receiver_seconds: f64,
}

/// What to do about the drift measured so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    /// Within tolerance. Do nothing.
    Hold,
    /// Sender is faster than the receiver consumes; audio is piling up.
    /// Drop this many frames to bring the buffer back to target.
    Drop(usize),
    /// Sender is slower; the buffer is draining. Insert this many frames.
    Insert(usize),
}

/// Tuning for [`DriftEstimator`].
#[derive(Debug, Clone, Copy)]
pub struct DriftConfig {
    /// Nominal sample rate of both clocks.
    pub sample_rate: u32,
    /// Drift below this magnitude is not worth acting on.
    pub deadband_ppm: f64,
    /// Buffer depth error, in frames, tolerated before correcting.
    pub depth_tolerance_frames: usize,
    /// Largest single correction, in frames.
    ///
    /// Corrections are deliberately small and frequent. Dropping a large block
    /// at once is audible; shedding a few frames spread over time is not.
    pub max_correction_frames: usize,

    /// Receiver-side gap that invalidates the history, in nanoseconds.
    ///
    /// A pause this long means the two clocks were not being compared across
    /// it: the phone slept, the route changed, the user walked out of range.
    /// The regression does not know that, and fitting a line through the gap
    /// measures the gap rather than the drift, producing an estimate that can
    /// be wrong by orders of magnitude and a correction that chases it.
    ///
    /// Two seconds is far longer than any jitter this project tolerates and
    /// far shorter than any pause a user would not notice.
    pub gap_reset_nanos: u64,
}

impl DriftConfig {
    /// Defaults for a 48 kHz stream.
    #[must_use]
    pub const fn for_rate(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            deadband_ppm: 2.0,
            depth_tolerance_frames: 240, // 5 ms at 48 kHz
            max_correction_frames: 48,   // 1 ms at 48 kHz
            gap_reset_nanos: 2_000_000_000,
        }
    }
}

/// Estimates the sender-to-receiver clock ratio by least squares.
#[derive(Debug)]
pub struct DriftEstimator {
    config: DriftConfig,
    observations: VecDeque<Observation>,
    /// Receiver time of the last observation, for gap detection.
    last_receiver_nanos: Option<u64>,
    /// Times the history has been discarded because of a gap.
    ///
    /// Worth reporting: a session that resets repeatedly never accumulates
    /// enough history to resolve anything, and the drift figure it shows is
    /// noise however confident it looks.
    resets: u64,
}

impl DriftEstimator {
    /// Create an estimator.
    #[must_use]
    pub fn new(config: DriftConfig) -> Self {
        Self {
            config,
            observations: VecDeque::with_capacity(WINDOW),
            last_receiver_nanos: None,
            resets: 0,
        }
    }

    /// Record that the sender had produced `sender_frames` when the receiver's
    /// monotonic clock read `receiver_nanos`.
    pub fn observe(&mut self, sender_frames: u64, receiver_nanos: u64) {
        // A long gap, or a clock that went backwards, means the history
        // describes a relationship that no longer holds. Keeping it would fit
        // a line across the discontinuity and call the result drift.
        if let Some(previous) = self.last_receiver_nanos {
            let moved_backwards = receiver_nanos < previous;
            let gap = receiver_nanos.saturating_sub(previous);
            if moved_backwards || gap >= self.config.gap_reset_nanos {
                self.observations.clear();
                self.resets += 1;
            }
        }
        self.last_receiver_nanos = Some(receiver_nanos);

        if self.observations.len() == WINDOW {
            self.observations.pop_front();
        }
        self.observations.push_back(Observation {
            sender_frames: sender_frames as f64,
            receiver_seconds: receiver_nanos as f64 / 1e9,
        });
    }

    /// Discard all history, for a new stream or a format change.
    pub fn reset(&mut self) {
        self.observations.clear();
        self.last_receiver_nanos = None;
    }

    /// Times the history has been discarded because of a gap in arrivals.
    #[must_use]
    pub const fn resets(&self) -> u64 {
        self.resets
    }

    /// Observations currently held.
    #[must_use]
    pub fn observation_count(&self) -> usize {
        self.observations.len()
    }

    /// Estimated sender clock rate in frames per second, or `None` before
    /// enough observations have accumulated.
    ///
    /// This is the slope of a least-squares fit of sender frames against
    /// receiver time.
    #[must_use]
    pub fn estimated_rate(&self) -> Option<f64> {
        if self.observations.len() < MIN_OBSERVATIONS {
            return None;
        }

        let n = self.observations.len() as f64;
        // Shift the origin to the first observation. Fitting raw monotonic
        // timestamps loses precision badly: they can be large, and squaring
        // them in the sum of squares makes it worse.
        let base = self.observations.front()?;

        let (mut sum_x, mut sum_y, mut sum_xx, mut sum_xy) = (0.0, 0.0, 0.0, 0.0);
        for observation in &self.observations {
            let x = observation.receiver_seconds - base.receiver_seconds;
            let y = observation.sender_frames - base.sender_frames;
            sum_x += x;
            sum_y += y;
            sum_xx += x * x;
            sum_xy += x * y;
        }

        let denominator = n * sum_xx - sum_x * sum_x;
        if denominator.abs() < f64::EPSILON {
            return None;
        }
        Some((n * sum_xy - sum_x * sum_y) / denominator)
    }

    /// Drift in parts per million, positive when the sender runs fast.
    ///
    /// `None` until enough observations exist.
    #[must_use]
    pub fn drift_ppm(&self) -> Option<f64> {
        let rate = self.estimated_rate()?;
        let nominal = f64::from(self.config.sample_rate);
        if nominal <= 0.0 {
            return None;
        }
        Some((rate / nominal - 1.0) * 1e6)
    }

    /// Smallest drift this estimator can currently resolve, in ppm.
    ///
    /// Derived from the window actually held and the spread of the residuals,
    /// so it reflects the link as it really is rather than an assumption. A
    /// `drift_ppm` smaller in magnitude than this is noise, not a measurement.
    ///
    /// Returns `None` before there are enough observations, or when the window
    /// spans no time at all.
    #[must_use]
    pub fn resolution_ppm(&self) -> Option<f64> {
        let rate = self.estimated_rate()?;
        let base = self.observations.front()?;
        let last = self.observations.back()?;

        let span_seconds = last.receiver_seconds - base.receiver_seconds;
        if span_seconds <= 0.0 {
            return None;
        }

        // Residual spread about the fitted line, expressed as a time.
        let n = self.observations.len() as f64;
        let mut sum_squared = 0.0;
        for observation in &self.observations {
            let x = observation.receiver_seconds - base.receiver_seconds;
            let y = observation.sender_frames - base.sender_frames;
            let residual_frames = y - rate * x;
            sum_squared += residual_frames * residual_frames;
        }
        let residual_frames = (sum_squared / n).sqrt();
        let residual_seconds = residual_frames / f64::from(self.config.sample_rate);

        // Slope uncertainty falls as 1/sqrt(N); express it back as ppm.
        Some(residual_seconds / (span_seconds * n.sqrt()) * 1e6)
    }

    /// Decide what correction, if any, the current state calls for.
    ///
    /// `depth_frames` is what the buffer holds now and `target_frames` what it
    /// should hold. The depth error is what actually gets corrected; the drift
    /// estimate is used as a gate, so that transient jitter does not trigger
    /// resampling decisions.
    #[must_use]
    pub fn correction(&self, depth_frames: usize, target_frames: usize) -> Correction {
        let Some(ppm) = self.drift_ppm() else {
            return Correction::Hold;
        };
        if ppm.abs() < self.config.deadband_ppm {
            return Correction::Hold;
        }

        let error = depth_frames as i64 - target_frames as i64;
        if error.unsigned_abs() as usize <= self.config.depth_tolerance_frames {
            return Correction::Hold;
        }

        let magnitude = (error.unsigned_abs() as usize).min(self.config.max_correction_frames);
        if error > 0 {
            Correction::Drop(magnitude)
        } else {
            Correction::Insert(magnitude)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn estimator() -> DriftEstimator {
        DriftEstimator::new(DriftConfig::for_rate(RATE))
    }

    /// Enough observations for the regression to resolve single-digit ppm.
    const LONG: usize = 3_000;

    /// Feed `count` packets from a sender whose true rate is `actual_rate`.
    fn feed(estimator: &mut DriftEstimator, count: usize, actual_rate: f64, frames: u64) {
        for n in 0..count as u64 {
            let sender_frames = n * frames;
            // The sender emits `frames` per packet on its own clock, so on the
            // receiver's clock that takes frames / actual_rate seconds.
            let seconds = sender_frames as f64 / actual_rate;
            estimator.observe(sender_frames, (seconds * 1e9) as u64);
        }
    }

    #[test]
    fn a_long_pause_discards_the_history() {
        // The phone slept, or the route changed. The regression cannot see
        // that; fitting a line across the gap measures the gap and calls it
        // drift, and the correction then chases a number that describes
        // nothing.
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE), 288);
        assert!(estimator.drift_ppm().is_some());

        let after_gap = LONG as u64 * 288 * 1_000_000_000 / u64::from(RATE) + 5_000_000_000;
        estimator.observe(LONG as u64 * 288, after_gap);

        assert_eq!(estimator.observation_count(), 1, "the history survived");
        assert_eq!(estimator.resets(), 1);
        assert!(
            estimator.drift_ppm().is_none(),
            "an estimate was offered from one observation"
        );
    }

    #[test]
    fn a_gap_shorter_than_the_threshold_keeps_the_history() {
        // Ordinary jitter must not throw away twenty-five seconds of history;
        // that would leave the estimator permanently unable to resolve
        // anything on a link that stutters.
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE), 288);
        let before = estimator.observation_count();

        let nominal = LONG as u64 * 288 * 1_000_000_000 / u64::from(RATE);
        estimator.observe(LONG as u64 * 288, nominal + 500_000_000);

        assert_eq!(estimator.resets(), 0);
        assert!(estimator.observation_count() >= before.min(WINDOW - 1));
        assert!(estimator.drift_ppm().is_some());
    }

    #[test]
    fn a_clock_that_moves_backwards_discards_the_history() {
        // Should be impossible from a monotonic source, which is why it is
        // worth checking: if one ever appears, the alternative is a negative
        // time delta and an estimate that is arbitrarily wrong.
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE), 288);

        estimator.observe(0, 0);

        assert_eq!(estimator.resets(), 1);
        assert_eq!(estimator.observation_count(), 1);
    }

    #[test]
    fn an_explicit_reset_also_forgets_the_last_arrival_time() {
        // Otherwise the first observation of the next stream is compared
        // against the previous stream's clock and looks like a gap, spending a
        // reset for nothing.
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE), 288);

        estimator.reset();
        estimator.observe(0, 0);

        assert_eq!(estimator.resets(), 0, "a spurious gap was detected");
    }

    #[test]
    fn no_estimate_before_enough_observations() {
        let mut estimator = estimator();
        feed(&mut estimator, MIN_OBSERVATIONS - 1, f64::from(RATE), 288);
        assert!(estimator.drift_ppm().is_none());
        assert_eq!(estimator.correction(0, 1440), Correction::Hold);
    }

    #[test]
    fn matched_clocks_measure_no_drift() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE), 288);

        let ppm = estimator.drift_ppm().expect("estimate");
        assert!(ppm.abs() < 0.5, "expected ~0 ppm, got {ppm}");
    }

    #[test]
    fn a_fast_sender_reads_as_positive_ppm() {
        let mut estimator = estimator();
        // Sender genuinely runs 50 ppm fast.
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 + 50e-6), 288);

        let ppm = estimator.drift_ppm().expect("estimate");
        assert!(
            (ppm - 50.0).abs() < 2.0,
            "expected about +50 ppm, got {ppm}"
        );
    }

    #[test]
    fn a_slow_sender_reads_as_negative_ppm() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 - 30e-6), 288);

        let ppm = estimator.drift_ppm().expect("estimate");
        assert!(
            (ppm + 30.0).abs() < 2.0,
            "expected about -30 ppm, got {ppm}"
        );
    }

    #[test]
    fn jitter_does_not_corrupt_the_slope() {
        // Same 40 ppm drift, but every arrival is displaced by up to +/- 3 ms
        // in a repeating pattern. Least squares should average that away.
        let mut estimator = estimator();
        let actual = f64::from(RATE) * (1.0 + 40e-6);
        let wobble = [0_i64, 3_000_000, -2_000_000, 1_000_000, -3_000_000];

        for n in 0..LONG as u64 {
            let sender_frames = n * 288;
            let clean = (sender_frames as f64 / actual * 1e9) as i64;
            let arrival = (clean + wobble[(n % 5) as usize]).max(0) as u64;
            estimator.observe(sender_frames, arrival);
        }

        let ppm = estimator.drift_ppm().expect("estimate");
        assert!(
            (ppm - 40.0).abs() < 5.0,
            "jitter should not move the slope much, got {ppm}"
        );
    }

    #[test]
    fn the_window_lets_the_estimate_track_a_change() {
        let mut estimator = estimator();
        feed(&mut estimator, WINDOW, f64::from(RATE) * (1.0 + 80e-6), 288);
        assert!(estimator.drift_ppm().unwrap() > 60.0);

        // A completely new stream at a different rate; enough observations to
        // flush the window entirely.
        estimator.reset();
        feed(&mut estimator, WINDOW, f64::from(RATE) * (1.0 - 80e-6), 288);
        assert!(
            estimator.drift_ppm().unwrap() < -60.0,
            "estimate should follow the new rate"
        );
    }

    #[test]
    fn the_window_is_bounded() {
        let mut estimator = estimator();
        feed(&mut estimator, WINDOW * 3, f64::from(RATE), 288);
        assert_eq!(estimator.observation_count(), WINDOW);
    }

    #[test]
    fn no_correction_inside_the_deadband() {
        let mut estimator = estimator();
        // 1 ppm, under the 2 ppm deadband.
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 + 1e-6), 288);
        assert_eq!(estimator.correction(9_999, 1_440), Correction::Hold);
    }

    #[test]
    fn no_correction_while_depth_is_close_to_target() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 + 50e-6), 288);
        // 100 frames of error, inside the 240 frame tolerance.
        assert_eq!(estimator.correction(1_540, 1_440), Correction::Hold);
    }

    #[test]
    fn an_overfull_buffer_is_corrected_by_dropping() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 + 50e-6), 288);
        assert_eq!(estimator.correction(2_440, 1_440), Correction::Drop(48));
    }

    #[test]
    fn a_draining_buffer_is_corrected_by_inserting() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 - 50e-6), 288);
        assert_eq!(estimator.correction(440, 1_440), Correction::Insert(48));
    }

    #[test]
    fn corrections_are_capped() {
        let mut estimator = estimator();
        feed(&mut estimator, LONG, f64::from(RATE) * (1.0 + 50e-6), 288);
        // A huge error must still produce at most max_correction_frames.
        assert_eq!(estimator.correction(500_000, 1_440), Correction::Drop(48));
    }

    #[test]
    fn twenty_ppm_matches_the_arithmetic_in_the_module_docs() {
        // 48000 * 20e-6 == 0.96 frames per second, 3456 per hour.
        let drift_frames_per_second = f64::from(RATE) * 20e-6;
        assert!((drift_frames_per_second - 0.96).abs() < 1e-9);
        assert!((drift_frames_per_second * 3600.0 - 3456.0).abs() < 1e-6);
    }
}
