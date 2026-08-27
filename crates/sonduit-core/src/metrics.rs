//! Instrumentation.
//!
//! Wired in from the start rather than retrofitted: a latency budget nobody
//! measures is a wish. Every stage of the pipeline reports into a
//! [`Telemetry`] snapshot, which the desktop UI renders directly.
//!
//! Counters are plain integers updated on the owning thread. Nothing here is
//! atomic, because the audio callback must not contend on anything; the
//! transport layer decides how a snapshot crosses threads.

use crate::jitter::JitterStats;

/// A monotonic counter of a single event class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Datagrams handed to the decoder.
    pub packets_received: u64,
    /// Datagrams rejected before decoding, for length, magic or version.
    pub packets_malformed: u64,
    /// Datagrams sent.
    pub packets_sent: u64,
    /// Audio frames written to the sink.
    pub frames_played: u64,
    /// Frames of silence emitted because the buffer was empty.
    pub frames_underrun: u64,
    /// Frames dropped by drift correction.
    pub frames_dropped_for_drift: u64,
    /// Frames inserted by drift correction.
    pub frames_inserted_for_drift: u64,
}

/// A point-in-time view of the pipeline, suitable for display.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Telemetry {
    /// Event counters.
    pub counters: Counters,
    /// Jitter buffer counters.
    pub jitter: JitterStats,
    /// Current buffer depth in milliseconds.
    pub buffer_depth_ms: f64,
    /// Depth the buffer is aiming for, in milliseconds.
    pub buffer_target_ms: f64,
    /// RFC 3550 inter-arrival jitter estimate, in milliseconds.
    pub jitter_ms: f64,
    /// Estimated clock drift in parts per million, if known yet.
    pub drift_ppm: Option<f64>,
}

impl Telemetry {
    /// Fraction of expected packets that never arrived, in percent.
    ///
    /// Returns zero before any packet has been seen, rather than a NaN that
    /// would propagate into the UI.
    #[must_use]
    pub fn packet_loss_percent(&self) -> f64 {
        let expected = self.jitter.accepted + self.jitter.lost;
        if expected == 0 {
            return 0.0;
        }
        self.jitter.lost as f64 * 100.0 / expected as f64
    }

    /// Fraction of played frames that were silence covering an underrun.
    #[must_use]
    pub fn underrun_percent(&self) -> f64 {
        if self.counters.frames_played == 0 {
            return 0.0;
        }
        self.counters.frames_underrun as f64 * 100.0 / self.counters.frames_played as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_zero_before_anything_happens() {
        let telemetry = Telemetry::default();
        assert_eq!(telemetry.packet_loss_percent(), 0.0);
        assert_eq!(telemetry.underrun_percent(), 0.0);
    }

    #[test]
    fn packet_loss_is_measured_against_expected_not_received() {
        let telemetry = Telemetry {
            jitter: JitterStats {
                accepted: 99,
                lost: 1,
                ..JitterStats::default()
            },
            ..Telemetry::default()
        };
        assert!((telemetry.packet_loss_percent() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn underrun_percentage_is_a_share_of_frames_played() {
        let telemetry = Telemetry {
            counters: Counters {
                frames_played: 1_000,
                frames_underrun: 25,
                ..Counters::default()
            },
            ..Telemetry::default()
        };
        assert!((telemetry.underrun_percent() - 2.5).abs() < 1e-9);
    }
}
