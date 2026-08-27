//! Turning a drift measurement into a resample ratio.
//!
//! [`crate::drift::DriftEstimator`] says how fast the sender's clock runs
//! relative to the receiver's. Knowing that changes nothing on its own: at
//! 50 ppm with 30 ms of headroom the buffer runs dry in ten minutes, so
//! something has to act on it.
//!
//! # Why a controller rather than the raw estimate
//!
//! Resampling by exactly the measured ratio corrects the *rate* but not the
//! *error already accumulated*. A buffer that has drifted 20 ms shallow stays
//! 20 ms shallow forever, one hiccup away from underrunning. So this is a PI
//! controller: the proportional term chases the depth back to target, and the
//! integral term settles onto the true rate ratio so the proportional term can
//! return to zero.
//!
//! # Why it ramps
//!
//! A step change in resample ratio is a step change in pitch, and a step in
//! pitch is audible in a way a slow glide is not. The output is rate-limited,
//! so the ratio moves smoothly even when the controller wants it somewhere
//! else immediately.

/// How far the ratio may stray from 1.0.
///
/// Crystal drift between consumer devices is tens of ppm; 500 ppm is far
/// beyond anything physical and is a clamp against a broken measurement, not a
/// tuning parameter. Half a cent of pitch, inaudible even if it were reached.
const MAX_DEVIATION: f64 = 500e-6;

/// Largest change in ratio per update, in parts per million.
///
/// At four updates a second this glides across the whole range in about two
/// seconds, which is slow enough not to be heard as pitch movement.
const MAX_SLEW_PPM: f64 = 60.0;

/// Tuning for [`RatioController`].
#[derive(Debug, Clone, Copy)]
pub struct RatioConfig {
    /// How hard depth error pulls the ratio, in ppm per millisecond of error.
    ///
    /// Too high oscillates; too low leaves the buffer sitting off target long
    /// enough for a burst of jitter to empty it.
    pub proportional_ppm_per_ms: f64,

    /// How fast the integral term accumulates, as a fraction of the
    /// proportional response per update.
    ///
    /// The integral is what actually cancels drift: it converges on the true
    /// rate ratio and holds it, letting the proportional term relax to zero.
    pub integral_gain: f64,

    /// Depth error, in milliseconds, small enough to ignore.
    ///
    /// Without a deadband the controller chases jitter, which is noise rather
    /// than drift, and never settles.
    pub deadband_ms: f64,

    /// Ceiling on the integral term, in ppm.
    ///
    /// A buffer that cannot reach target because something else is wrong would
    /// otherwise wind the integral up until the clamp, and unwinding it after
    /// the fault clears takes as long as winding it up did.
    pub integral_limit_ppm: f64,
}

impl Default for RatioConfig {
    fn default() -> Self {
        Self {
            proportional_ppm_per_ms: 4.0,
            integral_gain: 0.02,
            deadband_ms: 1.0,
            integral_limit_ppm: 200.0,
        }
    }
}

/// Produces a smoothly varying resample ratio from buffer depth and drift.
///
/// The ratio is output frames per input frame: greater than one means the
/// receiver is producing more audio than it takes in, which fills a buffer
/// that is running shallow.
#[derive(Debug)]
pub struct RatioController {
    config: RatioConfig,
    /// Accumulated correction, in ppm.
    integral_ppm: f64,
    /// What the controller last asked for, before slew limiting.
    target_ppm: f64,
    /// What is actually in effect.
    current_ppm: f64,
}

impl RatioController {
    /// A controller starting at unity.
    #[must_use]
    pub fn new(config: RatioConfig) -> Self {
        Self {
            config,
            integral_ppm: 0.0,
            target_ppm: 0.0,
            current_ppm: 0.0,
        }
    }

    /// The ratio to resample by right now.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        1.0 + self.current_ppm * 1e-6
    }

    /// The correction currently applied, in parts per million.
    #[must_use]
    pub const fn correction_ppm(&self) -> f64 {
        self.current_ppm
    }

    /// Whether the accumulated correction has reached its bound.
    ///
    /// The limit is far beyond any real crystal difference, so this means the
    /// buffer is not responding to correction: something other than drift is
    /// emptying or filling it. Worth surfacing rather than correcting.
    #[must_use]
    pub fn saturated(&self) -> bool {
        self.integral_ppm.abs() >= self.config.integral_limit_ppm - f64::EPSILON
    }

    /// Fold in a new reading.
    ///
    /// `depth_ms` is what the buffer holds, `target_ms` what it should hold,
    /// and `drift_ppm` the measured clock difference if one is available yet.
    /// The drift feeds the integral directly as a starting point, so the
    /// controller does not have to discover from scratch what has already been
    /// measured.
    pub fn update(&mut self, depth_ms: f64, target_ms: f64, drift_ppm: Option<f64>) {
        if !depth_ms.is_finite() || !target_ms.is_finite() {
            // A non-finite reading means something upstream is broken. Holding
            // the last good ratio is strictly better than propagating a NaN
            // into the resampler, where it would produce silence or noise.
            return;
        }

        let error_ms = depth_ms - target_ms;
        let effective_error = if error_ms.abs() <= self.config.deadband_ms {
            0.0
        } else {
            error_ms - self.config.deadband_ms * error_ms.signum()
        };

        // A buffer running shallow needs the receiver to consume more slowly,
        // which means producing more output per input frame: ratio above one.
        // Hence the negated error.
        let proportional = -effective_error * self.config.proportional_ppm_per_ms;

        self.integral_ppm += proportional * self.config.integral_gain;

        // The measured drift is the rate the integral is converging on anyway.
        // Seeding it removes the minutes the integral would otherwise take to
        // find a number that has already been measured.
        if let Some(ppm) = drift_ppm {
            if ppm.is_finite() && self.integral_ppm.abs() < f64::EPSILON {
                self.integral_ppm = -ppm;
            }
        }

        self.integral_ppm = self.integral_ppm.clamp(
            -self.config.integral_limit_ppm,
            self.config.integral_limit_ppm,
        );

        let limit = MAX_DEVIATION * 1e6;
        self.target_ppm = (proportional + self.integral_ppm).clamp(-limit, limit);

        let step = (self.target_ppm - self.current_ppm).clamp(-MAX_SLEW_PPM, MAX_SLEW_PPM);
        self.current_ppm += step;
    }

    /// Return to unity and forget the accumulated correction.
    ///
    /// Called on a route change, a resumed session or a format change: the
    /// clock relationship after one of those has nothing to do with the one
    /// before it, and carrying the integral across would correct a drift that
    /// no longer exists.
    pub fn reset(&mut self) {
        self.integral_ppm = 0.0;
        self.target_ppm = 0.0;
        self.current_ppm = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> RatioController {
        RatioController::new(RatioConfig::default())
    }

    /// Run the controller to steady state at a fixed depth error.
    fn settle(controller: &mut RatioController, depth_ms: f64, target_ms: f64, updates: usize) {
        for _ in 0..updates {
            controller.update(depth_ms, target_ms, None);
        }
    }

    #[test]
    fn a_buffer_at_target_asks_for_no_correction() {
        let mut controller = controller();
        settle(&mut controller, 30.0, 30.0, 100);
        assert_eq!(controller.ratio(), 1.0);
    }

    #[test]
    fn small_errors_inside_the_deadband_are_ignored() {
        // Jitter moves the depth by a millisecond constantly. Chasing it would
        // mean the ratio never settles, which is pitch movement for no gain.
        let mut controller = controller();
        settle(&mut controller, 30.5, 30.0, 50);
        assert_eq!(controller.correction_ppm(), 0.0);
    }

    #[test]
    fn a_shallow_buffer_slows_consumption() {
        // Less audio held than wanted means output must be stretched, which is
        // a ratio above one.
        let mut controller = controller();
        settle(&mut controller, 20.0, 30.0, 20);
        assert!(controller.ratio() > 1.0, "ratio was {}", controller.ratio());
    }

    #[test]
    fn a_deep_buffer_speeds_consumption() {
        let mut controller = controller();
        settle(&mut controller, 40.0, 30.0, 20);
        assert!(controller.ratio() < 1.0, "ratio was {}", controller.ratio());
    }

    #[test]
    fn the_ratio_never_steps_further_than_the_slew_limit() {
        // A step in ratio is a step in pitch, which is audible where a glide
        // is not.
        let mut controller = controller();
        let mut previous = controller.correction_ppm();
        for _ in 0..40 {
            controller.update(0.0, 30.0, None);
            let step = (controller.correction_ppm() - previous).abs();
            assert!(step <= MAX_SLEW_PPM + 1e-9, "stepped {step} ppm");
            previous = controller.correction_ppm();
        }
    }

    #[test]
    fn the_correction_never_exceeds_the_physical_range() {
        // Anything beyond a few hundred ppm is a broken measurement, not a
        // crystal.
        let mut controller = controller();
        settle(&mut controller, 0.0, 1_000.0, 500);
        assert!(
            controller.correction_ppm().abs() <= MAX_DEVIATION * 1e6 + 1e-9,
            "ran to {} ppm",
            controller.correction_ppm()
        );
    }

    #[test]
    fn the_integral_settles_on_a_steady_drift_and_the_error_returns_to_zero() {
        // This is the whole point. A controller with only a proportional term
        // holds the buffer at a constant offset from target forever, one burst
        // of jitter away from underrunning; the integral is what lets the
        // depth actually come back.
        let mut controller = controller();

        // A sender 40 ppm fast fills the buffer, and the applied correction
        // moves it the other way. Steady state is therefore correction = -40,
        // where the two cancel. The plant gain is arbitrary; only the signs
        // and the fixed point matter.
        let mut depth = 30.0;
        for _ in 0..2_000 {
            controller.update(depth, 30.0, None);
            depth += (40.0 + controller.correction_ppm()) * 0.002;
        }

        assert!(
            (depth - 30.0).abs() < 2.0,
            "depth settled at {depth} ms, not near the 30 ms target"
        );
        assert!(
            (controller.correction_ppm() + 40.0).abs() < 5.0,
            "correction settled at {} ppm, not near the -40 ppm that cancels the drift",
            controller.correction_ppm()
        );
    }

    #[test]
    fn a_measured_drift_seeds_the_integral_instead_of_being_rediscovered() {
        // The estimator takes 25 seconds to resolve a few ppm. Making the
        // controller find the same number again from zero would double that.
        //
        // Positive drift means the sender runs fast, so audio piles up and the
        // receiver has to consume faster: a ratio below one, a negative
        // correction. Getting this sign backwards would double the drift
        // instead of cancelling it.
        let mut controller = controller();
        controller.update(30.0, 30.0, Some(35.0));
        assert!(
            controller.correction_ppm() < 0.0,
            "expected a negative correction for a fast sender, got {}",
            controller.correction_ppm()
        );
    }

    #[test]
    fn a_seeded_integral_is_not_overwritten_on_every_update() {
        // Re-seeding each time would pin the integral to the raw estimate and
        // defeat the point of integrating at all.
        let mut controller = controller();
        controller.update(30.0, 30.0, Some(35.0));
        let after_seed = controller.correction_ppm();
        settle(&mut controller, 20.0, 30.0, 30);
        assert!(
            controller.correction_ppm() != after_seed,
            "the controller stopped responding once seeded"
        );
    }

    #[test]
    fn the_integral_is_bounded_rather_than_growing_without_limit() {
        // A buffer that cannot reach target, because the link is broken rather
        // than because the clocks differ, would otherwise wind the integral up
        // forever.
        let mut controller = controller();
        settle(&mut controller, 0.0, 500.0, 2_000);

        assert!(controller.saturated());
        assert!(
            controller.correction_ppm().abs() <= MAX_DEVIATION * 1e6 + 1e-9,
            "the clamp did not hold: {} ppm",
            controller.correction_ppm()
        );
    }

    #[test]
    fn a_wound_up_integral_unwinds_when_the_error_reverses() {
        // Without this, a fault that pins the integral leaves the correction
        // stuck at the bound long after the fault has cleared, and the audio
        // is then wrong in the opposite direction.
        let mut controller = controller();
        settle(&mut controller, 0.0, 500.0, 2_000);
        assert!(controller.saturated());

        // Driven the other way it has to come back through zero, and quickly:
        // an integral that needs as long to unwind as it took to wind up leaves
        // the audio wrong in the opposite direction for just as long.
        let mut updates = 0;
        while controller.correction_ppm() > 0.0 && updates < 200 {
            controller.update(500.0, 0.0, None);
            updates += 1;
        }

        assert!(
            controller.correction_ppm() <= 0.0,
            "the integral never came back"
        );
        assert!(updates < 50, "unwinding took {updates} updates");
    }

    #[test]
    fn a_reset_returns_to_unity() {
        let mut controller = controller();
        settle(&mut controller, 10.0, 30.0, 50);
        assert!(controller.correction_ppm() != 0.0);

        controller.reset();
        assert_eq!(controller.ratio(), 1.0);
        assert!(!controller.saturated());
    }

    #[test]
    fn a_non_finite_reading_holds_the_last_good_ratio_rather_than_poisoning_it() {
        // A NaN reaching the resampler produces silence or noise, and nothing
        // downstream can tell which.
        let mut controller = controller();
        settle(&mut controller, 20.0, 30.0, 10);
        let good = controller.ratio();

        controller.update(f64::NAN, 30.0, None);
        assert_eq!(controller.ratio(), good);
        assert!(controller.ratio().is_finite());
    }
}
