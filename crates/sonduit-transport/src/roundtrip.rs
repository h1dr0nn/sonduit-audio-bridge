//! Turning an echoed timestamp back into a round trip.
//!
//! The sender notes when it put each packet on the wire. The receiver echoes
//! the timestamp of the last one it accepted. Subtracting gives the round trip
//! without either end interpreting the other's clock, which is the usual
//! reason a round-trip figure is quietly wrong.
//!
//! Kept free of clocks so it can be driven from a test: the caller supplies
//! the monotonic reading, exactly as `sonduit-core` does elsewhere.

use std::collections::VecDeque;

/// Sends remembered while waiting to be echoed.
///
/// At four reports a second and one packet every six milliseconds, a report
/// echoes something sent within the last few dozen packets. This is well past
/// that, so a report delayed by a bad link still finds its send time, and it
/// is small enough that the search is trivial.
const HISTORY: usize = 256;

/// Records when packets were sent and matches echoes back to them.
#[derive(Debug)]
pub struct RoundTrip {
    /// (timestamp_frames, monotonic nanoseconds at send).
    sent: VecDeque<(u32, u64)>,
    /// Smoothed round trip in milliseconds, once anything has been measured.
    smoothed_ms: Option<f64>,
    /// Most recent raw measurement, for a caller that wants the unsmoothed one.
    last_ms: Option<f64>,
    samples: u64,
}

impl Default for RoundTrip {
    fn default() -> Self {
        Self::new()
    }
}

impl RoundTrip {
    /// An estimator with nothing measured yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sent: VecDeque::with_capacity(HISTORY),
            smoothed_ms: None,
            last_ms: None,
            samples: 0,
        }
    }

    /// Note that a packet carrying `timestamp_frames` left at `now_nanos`.
    pub fn record_send(&mut self, timestamp_frames: u32, now_nanos: u64) {
        if self.sent.len() == HISTORY {
            self.sent.pop_front();
        }
        self.sent.push_back((timestamp_frames, now_nanos));
    }

    /// Fold in an echo, returning the round trip it implies.
    ///
    /// `None` when the echo matches nothing this estimator remembers, which
    /// happens after a reset or when a report was delayed past the history.
    /// Guessing a value there would be worse than showing nothing.
    pub fn observe_echo(&mut self, echo: u32, now_nanos: u64) -> Option<f64> {
        let (_, sent_at) = self
            .sent
            .iter()
            .rev()
            .find(|(timestamp, _)| *timestamp == echo)
            .copied()?;

        // A monotonic clock cannot go backwards, but the caller supplies this
        // and a mistake there should not become a negative latency.
        let elapsed_ms = now_nanos.saturating_sub(sent_at) as f64 / 1_000_000.0;

        self.last_ms = Some(elapsed_ms);
        self.samples += 1;

        // The same one-pole filter RFC 3550 uses for jitter. A single report
        // delayed behind a burst of traffic should move the displayed figure a
        // little, not redraw it.
        self.smoothed_ms = Some(match self.smoothed_ms {
            None => elapsed_ms,
            Some(previous) => previous + (elapsed_ms - previous) / 16.0,
        });

        Some(elapsed_ms)
    }

    /// Smoothed round trip in milliseconds, if anything has been measured.
    #[must_use]
    pub const fn round_trip_ms(&self) -> Option<f64> {
        self.smoothed_ms
    }

    /// The most recent raw measurement.
    #[must_use]
    pub const fn last_round_trip_ms(&self) -> Option<f64> {
        self.last_ms
    }

    /// How many echoes have been matched.
    #[must_use]
    pub const fn samples(&self) -> u64 {
        self.samples
    }

    /// Forget everything.
    ///
    /// Called when the session restarts or the receiver changes: a round trip
    /// measured against a different device is not a measurement of this one.
    pub fn reset(&mut self) {
        self.sent.clear();
        self.smoothed_ms = None;
        self.last_ms = None;
        self.samples = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn nothing_is_reported_before_an_echo_arrives() {
        // The whole point. A sender with no reply has measured nothing and
        // must say so rather than showing a plausible constant.
        let estimator = RoundTrip::new();
        assert_eq!(estimator.round_trip_ms(), None);
        assert_eq!(estimator.samples(), 0);
    }

    #[test]
    fn an_echo_measures_the_time_since_that_packet_was_sent() {
        let mut estimator = RoundTrip::new();
        estimator.record_send(1000, 0);
        estimator.record_send(2000, 6 * MS);

        assert_eq!(estimator.observe_echo(1000, 20 * MS), Some(20.0));
        assert_eq!(estimator.round_trip_ms(), Some(20.0));
    }

    #[test]
    fn an_echo_for_a_packet_never_sent_measures_nothing() {
        // After a reset, or a report delayed past the history. A guess here
        // would be indistinguishable from a measurement in the UI.
        let mut estimator = RoundTrip::new();
        estimator.record_send(1000, 0);

        assert_eq!(estimator.observe_echo(9999, 20 * MS), None);
        assert_eq!(estimator.round_trip_ms(), None);
    }

    #[test]
    fn the_reported_figure_is_smoothed_rather_than_following_every_report() {
        // One report delayed behind a burst should move the number a little,
        // not redraw it.
        let mut estimator = RoundTrip::new();
        estimator.record_send(1, 0);
        estimator.observe_echo(1, 20 * MS);

        estimator.record_send(2, 0);
        estimator.observe_echo(2, 200 * MS);

        let smoothed = estimator.round_trip_ms().unwrap();
        assert!(smoothed > 20.0, "the spike was ignored entirely");
        assert!(smoothed < 40.0, "the spike moved it to {smoothed}");
        assert_eq!(estimator.last_round_trip_ms(), Some(200.0));
    }

    #[test]
    fn a_clock_that_appears_to_go_backwards_reports_zero_not_a_negative() {
        let mut estimator = RoundTrip::new();
        estimator.record_send(1, 100 * MS);
        assert_eq!(estimator.observe_echo(1, 0), Some(0.0));
    }

    #[test]
    fn the_history_is_bounded_and_the_oldest_send_is_forgotten_first() {
        let mut estimator = RoundTrip::new();
        for index in 0..(HISTORY as u32 + 10) {
            estimator.record_send(index, u64::from(index) * MS);
        }

        assert_eq!(estimator.observe_echo(0, 0), None, "the oldest survived");
        assert!(
            estimator.observe_echo(HISTORY as u32 + 5, 0).is_some(),
            "a recent send was forgotten"
        );
    }

    #[test]
    fn a_repeated_timestamp_matches_the_most_recent_send() {
        // The sender's timestamp wraps every 25 hours at 48 kHz, so the same
        // value does come round again. Matching the older one would report a
        // round trip of about a day.
        let mut estimator = RoundTrip::new();
        estimator.record_send(500, 0);
        estimator.record_send(500, 100 * MS);

        assert_eq!(estimator.observe_echo(500, 110 * MS), Some(10.0));
    }

    #[test]
    fn a_reset_forgets_the_measurement_and_the_history() {
        let mut estimator = RoundTrip::new();
        estimator.record_send(1, 0);
        estimator.observe_echo(1, 20 * MS);
        assert!(estimator.round_trip_ms().is_some());

        estimator.reset();

        assert_eq!(estimator.round_trip_ms(), None);
        assert_eq!(estimator.samples(), 0);
        assert_eq!(estimator.observe_echo(1, 30 * MS), None);
    }
}
