//! What the receiver tells the sender.
//!
//! # Why this exists
//!
//! UDP does not acknowledge anything, so a sender that only watches its own
//! socket learns nothing at all. It cannot tell a phone playing audio from a
//! phone that is switched off, and every figure it shows is therefore about
//! itself: how long its own capture period is, how many datagrams its own
//! socket refused. Presented next to the word "connected" that reads as a
//! working session when nothing is listening.
//!
//! It was presented exactly that way, and it was wrong. A bridge with no
//! receiver reported "connected" at "16 ms" with "0% loss".
//!
//! The receiver therefore sends a small report back, and the sender says
//! nothing it has not been told.
//!
//! # Why not RTCP
//!
//! RTCP is the right answer for a general RTP implementation and the wrong one
//! here. It carries sender reports, source descriptions, compound packet
//! rules and NTP timestamp reconciliation, none of which this project needs,
//! and its round-trip calculation assumes the RTP timestamp mapping that
//! `SonduitPacket` deliberately does not have. Twenty bytes of the four
//! numbers the UI actually shows is a smaller thing to get right.

use crate::TransportError;

/// Magic prefix, distinct from both audio wire formats so a report can never
/// be mistaken for a packet of samples.
pub const FEEDBACK_MAGIC: [u8; 4] = *b"SDFB";

/// Feedback format version.
///
/// # Why the queue depth did not bump this
///
/// [`Feedback::queue_tenths_ms`] was added into the two reserved bytes the
/// first version already sent as zero, and the encoding is deliberately
/// off-by-one: a queue of `n` tenths is written as `n + 1`, so the zero an
/// older receiver sends decodes as `None` rather than as an empty queue. The
/// datagram is the same length, carries the same magic and the same version,
/// and every field before byte 30 is untouched.
///
/// So a sender built against version 1 reads a report from a receiver that
/// fills the field exactly as it always did and ignores the two bytes, and a
/// sender built against this reads a report from a version 1 receiver as a
/// depth with no queue figure attached. Bumping the version instead would have
/// made those two combinations refuse each other outright, over a field
/// neither of them needs in order to play audio.
pub const FEEDBACK_VERSION: u8 = 1;

/// Encoded size of a report.
pub const FEEDBACK_BYTES: usize = 34;

/// How often the receiver should send one, in milliseconds.
///
/// Four a second, matching the rate the desktop repaints its telemetry.
/// Faster would tell the UI nothing it can display; slower would make a
/// receiver that has gone quiet take too long to notice.
pub const FEEDBACK_INTERVAL_MS: u64 = 250;

/// How long the sender waits before deciding the receiver has gone.
///
/// Three missed reports. One missed report is a dropped datagram, which is
/// exactly what this transport expects; three in a row is a receiver that has
/// stopped, moved network or been switched off.
pub const FEEDBACK_TIMEOUT_MS: u64 = FEEDBACK_INTERVAL_MS * 3;

/// What the receiver knows and the sender does not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feedback {
    /// `timestamp_frames` of the most recent packet the receiver accepted.
    ///
    /// Echoing a field the audio packet already carries is what lets the round
    /// trip be measured without adding anything to the wire format: the sender
    /// noted when it sent that timestamp, so it can subtract. Neither end ever
    /// has to interpret the other's clock, which is the usual reason a
    /// round-trip measurement is wrong.
    pub echo: u32,

    /// Receiver-side milliseconds between that packet arriving and this report
    /// being sent.
    ///
    /// Subtracted from the round trip, so a receiver that batches its reports
    /// does not have its own delay charged to the network.
    pub hold_ms: u16,

    /// Packets the jitter buffer accepted since the session began.
    pub accepted: u64,

    /// Packets that never arrived, as the receiver counts them.
    ///
    /// This is the real loss figure. The sender's own count of refused
    /// datagrams says only what its socket refused to send.
    pub lost: u64,

    /// Jitter buffer depth in tenths of a millisecond.
    ///
    /// Tenths rather than a float: the whole point of this message is that it
    /// is small and unambiguous on the wire, and a tenth of a millisecond is
    /// finer than anything the UI shows.
    ///
    /// This is one of the two buffers audio crosses on the receiver, not all
    /// of them. Use [`Feedback::held_ms`] for latency arithmetic.
    pub depth_tenths_ms: u16,

    /// Depth of the receiver's audio hand-off queue, in tenths of a
    /// millisecond, or `None` from a receiver that does not report it.
    ///
    /// The jitter buffer is not the last thing audio waits in. Downstream of
    /// it is a ring between the receive thread and the audio callback, and a
    /// session was measured holding a steady 110 ms there against a 36 ms
    /// jitter buffer -- more than the entire USB latency budget, in a stage
    /// no report carried. A sender that added `depth_ms` to its own figures
    /// and called the result end to end was understating it by more than
    /// everything else it had counted.
    ///
    /// `None` means the receiver did not say, which is not the same as a queue
    /// that is empty: an empty queue is a receiver that is underrunning right
    /// now. See the encoding note on [`FEEDBACK_VERSION`].
    pub queue_tenths_ms: Option<u16>,

    /// True once the receiver has audio playing, as opposed to merely
    /// listening.
    pub playing: bool,
}

impl Feedback {
    /// Encode into `out`, which must be at least [`FEEDBACK_BYTES`] long.
    ///
    /// # Errors
    /// Returns [`TransportError::UnknownFormat`] when the buffer is too small.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, TransportError> {
        if out.len() < FEEDBACK_BYTES {
            return Err(TransportError::UnknownFormat(out.len()));
        }

        out[0..4].copy_from_slice(&FEEDBACK_MAGIC);
        out[4] = FEEDBACK_VERSION;
        out[5] = u8::from(self.playing);
        out[6..10].copy_from_slice(&self.echo.to_le_bytes());
        out[10..12].copy_from_slice(&self.hold_ms.to_le_bytes());
        out[12..20].copy_from_slice(&self.accepted.to_le_bytes());
        out[20..28].copy_from_slice(&self.lost.to_le_bytes());
        out[28..30].copy_from_slice(&self.depth_tenths_ms.to_le_bytes());
        // Off by one, so that the zero a version 1 receiver sends here is read
        // as "did not say" rather than as an empty queue. Saturating rather
        // than wrapping: a queue over six seconds deep is a broken session, and
        // reporting it as a small number would hide that.
        let queue = self
            .queue_tenths_ms
            .map_or(0, |tenths| tenths.saturating_add(1));
        out[30..32].copy_from_slice(&queue.to_le_bytes());
        // Reserved, zeroed. Two spare bytes still costs nothing and saves a
        // version bump the first time one more counter is wanted.
        out[32..34].fill(0);

        Ok(FEEDBACK_BYTES)
    }

    /// Decode a datagram, or `None` if it is not a report this build reads.
    #[must_use]
    pub fn decode(datagram: &[u8]) -> Option<Self> {
        if datagram.len() < FEEDBACK_BYTES || datagram[0..4] != FEEDBACK_MAGIC {
            return None;
        }
        if datagram[4] != FEEDBACK_VERSION {
            return None;
        }

        Some(Self {
            playing: datagram[5] != 0,
            echo: u32::from_le_bytes(datagram[6..10].try_into().ok()?),
            hold_ms: u16::from_le_bytes(datagram[10..12].try_into().ok()?),
            accepted: u64::from_le_bytes(datagram[12..20].try_into().ok()?),
            lost: u64::from_le_bytes(datagram[20..28].try_into().ok()?),
            depth_tenths_ms: u16::from_le_bytes(datagram[28..30].try_into().ok()?),
            queue_tenths_ms: u16::from_le_bytes(datagram[30..32].try_into().ok()?).checked_sub(1),
        })
    }

    /// Jitter buffer depth as milliseconds.
    ///
    /// One stage of the receiver, not the whole of it. [`Feedback::held_ms`]
    /// is the figure a latency estimate wants.
    #[must_use]
    pub fn depth_ms(&self) -> f64 {
        f64::from(self.depth_tenths_ms) / 10.0
    }

    /// Hand-off queue depth as milliseconds, if the receiver reported one.
    #[must_use]
    pub fn queue_ms(&self) -> Option<f64> {
        self.queue_tenths_ms.map(|tenths| f64::from(tenths) / 10.0)
    }

    /// Everything the receiver is holding, in milliseconds.
    ///
    /// The jitter buffer plus the hand-off queue. This is what belongs in a
    /// capture-to-ear estimate: audio waits in both, and until the queue was
    /// reported the sender's own figure omitted a stage worth more than the
    /// whole USB budget.
    ///
    /// A receiver that does not report its queue contributes only its buffer
    /// depth, so the result understates by that receiver's queue. That is a
    /// limitation of the peer rather than of the arithmetic, and it is exactly
    /// what every sender showed before the field existed.
    #[must_use]
    pub fn held_ms(&self) -> f64 {
        self.depth_ms() + self.queue_ms().unwrap_or(0.0)
    }

    /// Share of expected packets that never arrived, in percent.
    ///
    /// Zero before anything has been seen, rather than a NaN that would reach
    /// the UI and render as nothing at all.
    #[must_use]
    pub fn loss_percent(&self) -> f64 {
        let expected = self.accepted + self.lost;
        if expected == 0 {
            return 0.0;
        }
        self.lost as f64 * 100.0 / expected as f64
    }
}

/// One-way latency implied by a round trip, in milliseconds.
///
/// `round_trip_ms` is measured by the sender against its own clock, so the two
/// devices never have to agree on the time. The receiver's own delay before
/// answering is removed first, because it belongs to the receiver and not to
/// the network.
///
/// Halving assumes a symmetric path. That is a real assumption and it is
/// wrong on an asymmetric link, which is why the figure is described in the UI
/// as an estimate rather than a measurement.
#[must_use]
pub fn one_way_ms(round_trip_ms: f64, hold_ms: u16) -> f64 {
    let network = round_trip_ms - f64::from(hold_ms);
    // A clock that jumped, or a report held longer than the round trip, can
    // make this negative. Zero is the only honest floor.
    (network / 2.0).max(0.0)
}

/// Latency from capture to ear, as far as either end can account for it.
///
/// The sender's own share, plus the network, plus what the receiver is
/// holding. It does not include the receiver's output device, which no API
/// reports honestly; `docs/latency-budget.md` says so.
///
/// `receiver_depth_ms` is **everything** the receiver is holding, which is
/// [`Feedback::held_ms`] and not [`Feedback::depth_ms`]. Passing the jitter
/// buffer depth alone is the accounting defect this signature invites, and it
/// cost a measured session more than a hundred milliseconds it never showed.
#[must_use]
pub fn end_to_end_ms(send_side_ms: f64, one_way_ms: f64, receiver_depth_ms: f64) -> f64 {
    send_side_ms + one_way_ms + receiver_depth_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Feedback {
        Feedback {
            echo: 0xDEAD_BEEF,
            hold_ms: 7,
            accepted: 12_345,
            lost: 12,
            depth_tenths_ms: 284,
            queue_tenths_ms: Some(120),
            playing: true,
        }
    }

    #[test]
    fn a_report_round_trips() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();
        assert_eq!(Feedback::decode(&buffer), Some(report()));
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_written_past() {
        let mut buffer = [0_u8; FEEDBACK_BYTES - 1];
        assert!(report().encode(&mut buffer).is_err());
    }

    #[test]
    fn a_truncated_report_is_refused_rather_than_panicking() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();
        for length in 0..FEEDBACK_BYTES {
            assert_eq!(Feedback::decode(&buffer[..length]), None, "length {length}");
        }
    }

    #[test]
    fn an_audio_packet_is_never_read_as_a_report() {
        // Both travel on the same socket in the opposite direction, and a
        // report read as audio would be played as noise.
        use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
        use sonduit_core::packet::SonduitPacket;

        let pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
        let mut datagram = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 0,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        }
        .encode(&mut datagram)
        .unwrap();

        assert_eq!(Feedback::decode(&datagram), None);
        assert_eq!(crate::classify(&datagram), Some(crate::Wire::Sonduit));
    }

    #[test]
    fn a_report_is_not_classified_as_audio() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();
        assert_eq!(crate::classify(&buffer), None);
    }

    #[test]
    fn a_future_version_is_refused_rather_than_misread() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();
        buffer[4] = FEEDBACK_VERSION + 1;
        assert_eq!(Feedback::decode(&buffer), None);
    }

    #[test]
    fn loss_is_zero_before_anything_arrives_rather_than_a_nan() {
        let empty = Feedback {
            accepted: 0,
            lost: 0,
            ..report()
        };
        assert_eq!(empty.loss_percent(), 0.0);
    }

    #[test]
    fn loss_is_measured_against_what_was_expected() {
        let report = Feedback {
            accepted: 75,
            lost: 25,
            ..report()
        };
        assert_eq!(report.loss_percent(), 25.0);
    }

    #[test]
    fn the_receivers_own_delay_is_not_charged_to_the_network() {
        // A receiver that answers 10 ms after the packet landed has not added
        // 10 ms of network latency, and charging it would make a batching
        // receiver look like a slow link.
        assert_eq!(one_way_ms(30.0, 10), 10.0);
        assert_eq!(one_way_ms(30.0, 0), 15.0);
    }

    #[test]
    fn a_nonsensical_round_trip_reports_zero_rather_than_a_negative_latency() {
        // The clock can jump, and a report can be held longer than the round
        // trip appeared to take.
        assert_eq!(one_way_ms(5.0, 50), 0.0);
        assert_eq!(one_way_ms(-1.0, 0), 0.0);
    }

    #[test]
    fn end_to_end_adds_up_the_three_parts_that_are_known() {
        assert_eq!(end_to_end_ms(16.0, 4.0, 28.4), 48.4);
    }

    #[test]
    fn a_version_1_report_decodes_with_no_queue_rather_than_an_empty_one() {
        // Version 1 sent two zeroed reserved bytes here. Reading that as a
        // queue holding nothing would say the receiver is underrunning, which
        // is a different and much worse claim than saying nothing at all.
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();
        buffer[30..32].fill(0);

        let decoded = Feedback::decode(&buffer).unwrap();
        assert_eq!(decoded.queue_tenths_ms, None);
        assert_eq!(decoded.queue_ms(), None);
        // And the report still decodes, at the same version, with every other
        // field intact. A bumped version would have refused it outright.
        assert_eq!(decoded.depth_ms(), 28.4);
        assert_eq!(decoded.accepted, 12_345);
    }

    #[test]
    fn an_empty_queue_is_reported_as_empty_and_not_as_silence() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        Feedback {
            queue_tenths_ms: Some(0),
            ..report()
        }
        .encode(&mut buffer)
        .unwrap();

        assert_eq!(Feedback::decode(&buffer).unwrap().queue_ms(), Some(0.0));
    }

    #[test]
    fn a_version_1_sender_reads_every_field_it_knows_about_unchanged() {
        // The bytes a version 1 decoder looks at. Adding the queue must not
        // have moved or reinterpreted any of them, or a shipped sender starts
        // reading a loss count as a packet count.
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        report().encode(&mut buffer).unwrap();

        assert_eq!(buffer[0..4], FEEDBACK_MAGIC);
        assert_eq!(buffer[4], 1);
        assert_eq!(buffer[5], 1);
        assert_eq!(
            u32::from_le_bytes(buffer[6..10].try_into().unwrap()),
            0xDEAD_BEEF
        );
        assert_eq!(u16::from_le_bytes(buffer[10..12].try_into().unwrap()), 7);
        assert_eq!(
            u64::from_le_bytes(buffer[12..20].try_into().unwrap()),
            12_345
        );
        assert_eq!(u64::from_le_bytes(buffer[20..28].try_into().unwrap()), 12);
        assert_eq!(u16::from_le_bytes(buffer[28..30].try_into().unwrap()), 284);
        // Still 34 bytes, still two bytes spare for the next counter.
        assert_eq!(buffer[32..34], [0, 0]);
    }

    #[test]
    fn what_the_receiver_holds_is_both_buffers_and_not_one_of_them() {
        // The defect this field exists for: a session held 110 ms in the queue
        // behind a 42 ms buffer, and every figure the sender showed came from
        // the 42.
        let report = Feedback {
            depth_tenths_ms: 420,
            queue_tenths_ms: Some(1_100),
            ..report()
        };
        assert_eq!(report.depth_ms(), 42.0);
        assert_eq!(report.held_ms(), 152.0);
        assert_eq!(end_to_end_ms(16.0, 4.0, report.held_ms()), 172.0);
    }

    #[test]
    fn a_receiver_that_does_not_report_its_queue_counts_only_what_it_said() {
        // Understated, exactly as before the field existed, rather than
        // guessed at. A number invented here would be indistinguishable from a
        // measurement.
        let report = Feedback {
            depth_tenths_ms: 420,
            queue_tenths_ms: None,
            ..report()
        };
        assert_eq!(report.held_ms(), 42.0);
    }

    #[test]
    fn a_deep_queue_saturates_rather_than_wrapping_to_a_small_number() {
        // The off-by-one encoding must not turn the deepest reportable queue
        // into an absent one.
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        Feedback {
            queue_tenths_ms: Some(u16::MAX),
            ..report()
        }
        .encode(&mut buffer)
        .unwrap();

        let decoded = Feedback::decode(&buffer).unwrap().queue_tenths_ms;
        assert_eq!(decoded, Some(u16::MAX - 1), "wrapped to {decoded:?}");
    }

    #[test]
    fn depth_survives_the_tenths_encoding() {
        let mut buffer = [0_u8; FEEDBACK_BYTES];
        Feedback {
            depth_tenths_ms: 284,
            ..report()
        }
        .encode(&mut buffer)
        .unwrap();

        assert_eq!(Feedback::decode(&buffer).unwrap().depth_ms(), 28.4);
    }

    #[test]
    fn the_timeout_is_longer_than_one_missed_report() {
        // A single dropped datagram is what this transport is for. Declaring
        // the receiver gone on one would make the status flicker constantly.
        const { assert!(FEEDBACK_TIMEOUT_MS > FEEDBACK_INTERVAL_MS) };
    }
}
