//! Adaptive jitter buffer.
//!
//! Absorbs the difference between when packets arrive and when the audio
//! callback needs them. It reorders, detects loss and duplicates, and sizes
//! itself from the inter-arrival jitter estimator in RFC 3550 section 6.4.1.
//!
//! Nothing here reads a clock. Arrival times are passed in, which is what
//! makes the whole module testable against a synthetic timeline.

use std::collections::BTreeMap;

use crate::format::Format;

/// Half the sequence-number space, the point at which a jump is read as a wrap
/// rather than a large forward step.
const SEQUENCE_WRAP_HALF: i32 = 32_768;

/// Gain of the RFC 3550 jitter filter.
///
/// The RFC fixes this at 1/16: "the gain parameter 1/16 gives a good noise
/// reduction ratio while maintaining a reasonable rate of convergence".
const JITTER_GAIN: f64 = 1.0 / 16.0;

/// Rebuilds a monotonic 64-bit counter from wrapping 16-bit sequence numbers.
///
/// Reordering across a wrap boundary is the case that makes naive comparison
/// wrong: sequence 0xFFFF arriving after 0x0001 is one packet late, not
/// 65534 packets early.
#[derive(Debug, Default)]
struct SequenceExtender {
    highest: u64,
    started: bool,
}

impl SequenceExtender {
    fn extend(&mut self, sequence: u16) -> u64 {
        if !self.started {
            self.started = true;
            self.highest = u64::from(sequence);
            return self.highest;
        }

        let last = (self.highest & 0xFFFF) as u16;
        let base = self.highest - u64::from(last);
        let delta = i32::from(sequence) - i32::from(last);

        let candidate = if delta < -SEQUENCE_WRAP_HALF {
            // Wrapped forward past 0xFFFF.
            base as i64 + 65_536 + i64::from(sequence)
        } else if delta > SEQUENCE_WRAP_HALF {
            // A straggler from before the wrap.
            base as i64 - 65_536 + i64::from(sequence)
        } else {
            base as i64 + i64::from(sequence)
        };

        let candidate = candidate.max(0) as u64;
        if candidate > self.highest {
            self.highest = candidate;
        }
        candidate
    }
}

/// Tuning for [`JitterBuffer`].
#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    /// Depth the buffer aims for when the link is perfectly smooth.
    pub target_ms: u32,
    /// Floor on the adaptive depth.
    pub min_ms: u32,
    /// Ceiling on the adaptive depth. Also bounds worst-case added latency.
    pub max_ms: u32,
    /// Multiple of the estimated jitter to hold on top of one packet.
    ///
    /// Three standard deviations is the usual VoIP choice; the estimator is a
    /// mean absolute deviation rather than a true sigma, so this is a
    /// heuristic and not a probability bound.
    pub jitter_multiplier: f64,
    /// Packets held before the buffer refuses more, protecting against a
    /// sender that floods faster than the sink drains.
    pub max_packets: usize,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            target_ms: 30,
            min_ms: 10,
            max_ms: 200,
            jitter_multiplier: 3.0,
            max_packets: 256,
        }
    }
}

/// What [`JitterBuffer::push`] did with a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Stored and waiting to be played.
    Accepted,
    /// Same sequence number as one already held.
    Duplicate,
    /// Older than the packet the buffer has already released. Unusable.
    TooLate,
    /// The buffer is full; the packet was dropped rather than displacing audio
    /// that is closer to being played.
    Overflow,
}

/// What [`JitterBuffer::pop`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopOutcome {
    /// The next packet in sequence.
    Packet(Vec<u8>),
    /// The next packet never arrived and waiting longer would stall playback.
    ///
    /// The caller must conceal this, at minimum by emitting silence of one
    /// packet's duration.
    Lost,
    /// Nothing to play yet: either still filling to the target depth, or the
    /// sender has stopped.
    Starved,
}

/// Counters describing what the buffer has seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    /// Packets stored.
    pub accepted: u64,
    /// Packets discarded because an identical sequence number was already held.
    pub duplicates: u64,
    /// Packets that arrived after their slot had already been played.
    pub too_late: u64,
    /// Packets dropped because the buffer was full.
    pub overflows: u64,
    /// Slots given up on and concealed.
    pub lost: u64,
    /// Calls that found nothing to play.
    pub starved: u64,
    /// Packets that arrived out of order but early enough to be reordered.
    pub reordered: u64,
}

/// Reorders, conceals and paces incoming audio packets.
#[derive(Debug)]
pub struct JitterBuffer {
    format: Format,
    config: JitterConfig,
    entries: BTreeMap<u64, Vec<u8>>,
    extender: SequenceExtender,
    /// Extended sequence number the next [`JitterBuffer::pop`] will emit.
    next: Option<u64>,
    /// RFC 3550 jitter estimate, in frames.
    jitter_frames: f64,
    /// Previous accepted packet, as (arrival in frames, sender timestamp).
    ///
    /// Both are kept so the sender delta can be differenced with wrapping
    /// arithmetic. A single combined transit value cannot express that.
    previous_observation: Option<(i64, u32)>,
    /// True once the buffer has filled to its target and started playing.
    playing: bool,
    stats: JitterStats,
}

impl JitterBuffer {
    /// Create a buffer for `format`.
    #[must_use]
    pub fn new(format: Format, config: JitterConfig) -> Self {
        Self {
            format,
            config,
            entries: BTreeMap::new(),
            extender: SequenceExtender::default(),
            next: None,
            jitter_frames: 0.0,
            previous_observation: None,
            playing: false,
            stats: JitterStats::default(),
        }
    }

    /// Counters describing what the buffer has seen.
    #[must_use]
    pub const fn stats(&self) -> JitterStats {
        self.stats
    }

    /// Packets currently held.
    #[must_use]
    pub fn depth_packets(&self) -> usize {
        self.entries.len()
    }

    /// Current RFC 3550 jitter estimate, in milliseconds.
    #[must_use]
    pub fn jitter_ms(&self) -> f64 {
        self.jitter_frames * 1000.0 / f64::from(self.format.sample_rate)
    }

    /// Buffered audio, in milliseconds.
    #[must_use]
    pub fn depth_ms(&self) -> f64 {
        let frames = self.format.frames_per_packet().unwrap_or(0) as f64;
        self.depth_packets() as f64 * frames * 1000.0 / f64::from(self.format.sample_rate)
    }

    /// Depth the buffer is currently aiming for, in milliseconds.
    ///
    /// One packet, plus `jitter_multiplier` times the jitter estimate, floored
    /// at the configured target and clamped to the configured range.
    #[must_use]
    pub fn target_ms(&self) -> f64 {
        let packet_ms = self
            .format
            .packet_duration_nanos()
            .map_or(0.0, |nanos| nanos as f64 / 1_000_000.0);

        // JitterConfig is a plain public struct with no validation, and
        // f64::clamp panics outright when min > max. Order the bounds here
        // rather than trusting the caller; this runs near the audio path and
        // must not be able to panic.
        let low = f64::from(self.config.min_ms.min(self.config.max_ms));
        let high = f64::from(self.config.min_ms.max(self.config.max_ms));

        let adaptive = packet_ms + self.config.jitter_multiplier * self.jitter_ms();
        adaptive
            .max(f64::from(self.config.target_ms))
            .clamp(low, high)
    }

    fn target_packets(&self) -> usize {
        let packet_ms = self
            .format
            .packet_duration_nanos()
            .map_or(0.0, |nanos| nanos as f64 / 1_000_000.0);
        if packet_ms <= 0.0 {
            return 1;
        }
        ((self.target_ms() / packet_ms).ceil() as usize).max(1)
    }

    /// Update the RFC 3550 estimate from one packet's arrival.
    ///
    /// `D(i-1,i) = (Rj - Ri) - (Sj - Si)`, the change in transit time between
    /// consecutive packets, then `J += (|D| - J)/16`. Both terms are converted
    /// to frames so the units cancel.
    fn observe_arrival(&mut self, timestamp_frames: u32, arrival_nanos: u64) {
        let arrival_frames =
            (arrival_nanos as i128 * i128::from(self.format.sample_rate) / 1_000_000_000) as i64;

        if let Some((previous_arrival, previous_timestamp)) = self.previous_observation {
            let arrival_delta = arrival_frames - previous_arrival;

            // The sender timestamp is a u32 frame counter, so it wraps roughly
            // every 24.8 hours at 48 kHz. Differencing it as a plain integer
            // turns that wrap into a four-billion-frame jump and destroys the
            // estimate for the next ~90 packets. wrapping_sub reinterpreted as
            // i32 gives the true signed delta for any real spacing.
            let sender_delta = i64::from(timestamp_frames.wrapping_sub(previous_timestamp) as i32);

            let d = (arrival_delta - sender_delta).abs() as f64;
            self.jitter_frames += (d - self.jitter_frames) * JITTER_GAIN;
        }

        self.previous_observation = Some((arrival_frames, timestamp_frames));
    }

    /// Offer a packet to the buffer.
    ///
    /// `arrival_nanos` is a receiver-side monotonic timestamp. It is only ever
    /// differenced, so its epoch does not matter.
    pub fn push(
        &mut self,
        sequence: u16,
        timestamp_frames: u32,
        arrival_nanos: u64,
        pcm: Vec<u8>,
    ) -> PushOutcome {
        let extended = self.extender.extend(sequence);

        // The TooLate test only means anything once playback has started.
        // Latching `next` on the first *push* instead would reject every packet
        // that arrives out of order while the buffer is still filling, which is
        // exactly when reordering is most repairable.
        if let Some(next) = self.next {
            if extended < next {
                self.stats.too_late += 1;
                return PushOutcome::TooLate;
            }
        }

        if self.entries.contains_key(&extended) {
            self.stats.duplicates += 1;
            return PushOutcome::Duplicate;
        }

        if self.entries.len() >= self.config.max_packets {
            self.stats.overflows += 1;
            return PushOutcome::Overflow;
        }

        // Only packets the buffer actually keeps may move the jitter estimate.
        // Observing rejected ones lets a single stale or duplicate datagram
        // pin the target at max_ms for hundreds of packets.
        self.observe_arrival(timestamp_frames, arrival_nanos);

        // Arriving behind a packet already held means the network reordered it.
        if self
            .entries
            .last_key_value()
            .is_some_and(|(highest, _)| extended < *highest)
        {
            self.stats.reordered += 1;
        }

        self.entries.insert(extended, pcm);
        self.stats.accepted += 1;
        PushOutcome::Accepted
    }

    /// Take the next packet due for playback.
    pub fn pop(&mut self) -> PopOutcome {
        // Hold playback until the target depth is reached, then keep going
        // until the buffer genuinely empties. Re-arming on every dip would
        // stutter continuously on a link that merely runs close to the target.
        if !self.playing {
            if self.entries.len() < self.target_packets() {
                self.stats.starved += 1;
                return PopOutcome::Starved;
            }
            self.playing = true;

            // Playback starts at the LOWEST sequence held, which is only known
            // once filling has finished. Choosing it when the first packet was
            // pushed instead would pick whatever happened to arrive first and
            // then reject everything earlier as too late, destroying exactly
            // the reordering the buffer exists to repair.
            if self.next.is_none() {
                self.next = self.entries.keys().next().copied();
            }
        }

        let Some(next) = self.next else {
            self.stats.starved += 1;
            return PopOutcome::Starved;
        };

        if let Some(pcm) = self.entries.remove(&next) {
            self.next = Some(next + 1);
            return PopOutcome::Packet(pcm);
        }

        // The slot is empty. Give up on it only when something later is
        // already waiting; otherwise the packet may still be in flight and
        // concealing now would create a gap that did not need to exist.
        if self.entries.is_empty() {
            self.playing = false;
            self.stats.starved += 1;
            return PopOutcome::Starved;
        }

        self.next = Some(next + 1);
        self.stats.lost += 1;
        PopOutcome::Lost
    }

    /// Drop everything and re-arm, for a format change or a new sender.
    pub fn reset(&mut self) {
        self.entries.clear();
        self.extender = SequenceExtender::default();
        self.next = None;
        self.previous_observation = None;
        self.jitter_frames = 0.0;
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 288 frames at 48 kHz is 6 ms, so packet n is due at n * 6 ms.
    const PACKET_NANOS: u64 = 6_000_000;
    const PACKET_FRAMES: u32 = 288;

    fn buffer() -> JitterBuffer {
        JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                target_ms: 12,
                min_ms: 6,
                max_ms: 200,
                jitter_multiplier: 3.0,
                max_packets: 64,
            },
        )
    }

    fn pcm(tag: u8) -> Vec<u8> {
        vec![tag; 8]
    }

    /// Push packet `n` as if it arrived exactly on time.
    fn push_on_time(buffer: &mut JitterBuffer, n: u16) -> PushOutcome {
        buffer.push(
            n,
            u32::from(n) * PACKET_FRAMES,
            u64::from(n) * PACKET_NANOS,
            pcm(n as u8),
        )
    }

    fn drain(buffer: &mut JitterBuffer) -> Vec<PopOutcome> {
        let mut out = Vec::new();
        loop {
            match buffer.pop() {
                PopOutcome::Starved => return out,
                other => out.push(other),
            }
        }
    }

    #[test]
    fn a_clean_stream_comes_out_in_order() {
        let mut buffer = buffer();
        for n in 0..8 {
            assert_eq!(push_on_time(&mut buffer, n), PushOutcome::Accepted);
        }

        let popped = drain(&mut buffer);
        assert_eq!(popped.len(), 8);
        for (n, outcome) in popped.iter().enumerate() {
            assert_eq!(*outcome, PopOutcome::Packet(pcm(n as u8)));
        }
        assert_eq!(buffer.stats().lost, 0);
        assert_eq!(buffer.stats().reordered, 0);
    }

    #[test]
    fn nothing_plays_until_the_target_depth_is_reached() {
        let mut buffer = buffer();
        // Target is 12 ms, i.e. two 6 ms packets.
        push_on_time(&mut buffer, 0);
        assert_eq!(buffer.pop(), PopOutcome::Starved);

        push_on_time(&mut buffer, 1);
        assert_eq!(buffer.pop(), PopOutcome::Packet(pcm(0)));
    }

    #[test]
    fn out_of_order_packets_are_put_back_in_order() {
        let mut buffer = buffer();
        // Arrival order 0, 2, 1, 3 with the timestamps they should have had.
        for n in [0_u16, 2, 1, 3] {
            push_on_time(&mut buffer, n);
        }

        let popped = drain(&mut buffer);
        assert_eq!(
            popped,
            vec![
                PopOutcome::Packet(pcm(0)),
                PopOutcome::Packet(pcm(1)),
                PopOutcome::Packet(pcm(2)),
                PopOutcome::Packet(pcm(3)),
            ]
        );
        assert_eq!(buffer.stats().reordered, 1, "packet 1 arrived behind 2");
        assert_eq!(buffer.stats().lost, 0);
    }

    #[test]
    fn a_missing_packet_is_reported_lost_once_later_audio_is_waiting() {
        let mut buffer = buffer();
        for n in [0_u16, 1, 3, 4] {
            push_on_time(&mut buffer, n);
        }

        let popped = drain(&mut buffer);
        assert_eq!(
            popped,
            vec![
                PopOutcome::Packet(pcm(0)),
                PopOutcome::Packet(pcm(1)),
                PopOutcome::Lost,
                PopOutcome::Packet(pcm(3)),
                PopOutcome::Packet(pcm(4)),
            ]
        );
        assert_eq!(buffer.stats().lost, 1);
    }

    #[test]
    fn a_gap_is_not_declared_lost_while_it_could_still_arrive() {
        let mut buffer = buffer();
        push_on_time(&mut buffer, 0);
        push_on_time(&mut buffer, 1);

        assert_eq!(buffer.pop(), PopOutcome::Packet(pcm(0)));
        assert_eq!(buffer.pop(), PopOutcome::Packet(pcm(1)));
        // Packet 2 has not arrived and nothing later is waiting, so the buffer
        // must wait rather than conceal.
        assert_eq!(buffer.pop(), PopOutcome::Starved);
        assert_eq!(buffer.stats().lost, 0);

        push_on_time(&mut buffer, 2);
        push_on_time(&mut buffer, 3);
        assert_eq!(buffer.pop(), PopOutcome::Packet(pcm(2)));
    }

    #[test]
    fn duplicates_are_counted_and_dropped() {
        let mut buffer = buffer();
        push_on_time(&mut buffer, 0);
        assert_eq!(push_on_time(&mut buffer, 0), PushOutcome::Duplicate);
        push_on_time(&mut buffer, 1);

        assert_eq!(buffer.stats().duplicates, 1);
        assert_eq!(buffer.depth_packets(), 2);

        let popped = drain(&mut buffer);
        assert_eq!(popped.len(), 2);
    }

    #[test]
    fn a_packet_arriving_after_its_slot_played_is_rejected() {
        let mut buffer = buffer();
        for n in [0_u16, 1, 3, 4] {
            push_on_time(&mut buffer, n);
        }
        let _ = drain(&mut buffer);

        // Packet 2 finally shows up, long after the gap was concealed.
        assert_eq!(push_on_time(&mut buffer, 2), PushOutcome::TooLate);
        assert_eq!(buffer.stats().too_late, 1);
    }

    #[test]
    fn the_buffer_refuses_to_grow_without_bound() {
        let mut buffer = JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                max_packets: 4,
                ..JitterConfig::default()
            },
        );
        for n in 0..4 {
            assert_eq!(push_on_time(&mut buffer, n), PushOutcome::Accepted);
        }
        assert_eq!(push_on_time(&mut buffer, 4), PushOutcome::Overflow);
        assert_eq!(buffer.stats().overflows, 1);
    }

    #[test]
    fn sequence_numbers_wrap_without_reordering_the_stream() {
        let mut buffer = buffer();
        // Straddle the 16-bit boundary.
        let sequence: Vec<u16> = (0..6).map(|i| 65_533_u16.wrapping_add(i)).collect();
        for (index, seq) in sequence.iter().enumerate() {
            buffer.push(
                *seq,
                index as u32 * PACKET_FRAMES,
                index as u64 * PACKET_NANOS,
                pcm(index as u8),
            );
        }

        let popped = drain(&mut buffer);
        assert_eq!(popped.len(), 6, "every packet across the wrap must survive");
        for (index, outcome) in popped.iter().enumerate() {
            assert_eq!(*outcome, PopOutcome::Packet(pcm(index as u8)));
        }
        assert_eq!(buffer.stats().lost, 0);
        assert_eq!(buffer.stats().too_late, 0);
    }

    #[test]
    fn reordering_across_the_wrap_boundary_is_still_reordering() {
        let mut buffer = buffer();
        // 0xFFFF arrives after 0x0000, which naive comparison reads as a
        // 65535-packet jump backwards.
        buffer.push(65_534, 0, 0, pcm(0));
        buffer.push(0, 2 * PACKET_FRAMES, 2 * PACKET_NANOS, pcm(2));
        buffer.push(65_535, PACKET_FRAMES, PACKET_NANOS, pcm(1));

        let popped = drain(&mut buffer);
        assert_eq!(
            popped,
            vec![
                PopOutcome::Packet(pcm(0)),
                PopOutcome::Packet(pcm(1)),
                PopOutcome::Packet(pcm(2)),
            ]
        );
    }

    #[test]
    fn a_perfectly_paced_stream_estimates_almost_no_jitter() {
        let mut buffer = buffer();
        for n in 0..50 {
            push_on_time(&mut buffer, n);
            let _ = buffer.pop();
        }
        assert!(
            buffer.jitter_ms() < 0.01,
            "expected near-zero jitter, got {}",
            buffer.jitter_ms()
        );
    }

    #[test]
    fn jitter_estimate_rises_with_arrival_spread_and_target_follows() {
        let mut steady = buffer();
        let mut jittery = buffer();

        // Same nominal timeline; the jittery one has arrivals swinging by
        // +/- 5 ms around where they should be.
        for n in 0..80_u16 {
            let nominal = u64::from(n) * PACKET_NANOS;
            let wobble = if n % 2 == 0 { 5_000_000 } else { 0 };

            steady.push(n, u32::from(n) * PACKET_FRAMES, nominal, pcm(0));
            jittery.push(n, u32::from(n) * PACKET_FRAMES, nominal + wobble, pcm(0));
            let _ = steady.pop();
            let _ = jittery.pop();
        }

        assert!(
            jittery.jitter_ms() > steady.jitter_ms() + 1.0,
            "jittery {} should exceed steady {} by a clear margin",
            jittery.jitter_ms(),
            steady.jitter_ms()
        );
        assert!(
            jittery.target_ms() > steady.target_ms(),
            "a jittery link must ask for a deeper buffer"
        );
    }

    #[test]
    fn the_target_never_leaves_the_configured_range() {
        let config = JitterConfig {
            target_ms: 12,
            min_ms: 6,
            max_ms: 40,
            jitter_multiplier: 3.0,
            max_packets: 512,
        };
        let mut buffer = JitterBuffer::new(Format::stereo_48k(), config);

        // Arrivals scattered violently enough to drive the estimate up hard.
        for n in 0..300_u16 {
            let spike = if n % 3 == 0 { 90_000_000 } else { 0 };
            buffer.push(
                n,
                u32::from(n) * PACKET_FRAMES,
                u64::from(n) * PACKET_NANOS + spike,
                pcm(0),
            );
            let _ = buffer.pop();
        }

        let target = buffer.target_ms();
        assert!(
            target <= f64::from(config.max_ms),
            "target {target} exceeded the ceiling"
        );
        assert!(
            target >= f64::from(config.min_ms),
            "target {target} fell below the floor"
        );
    }

    #[test]
    fn a_steadily_skewing_sender_clock_does_not_inflate_the_jitter_estimate() {
        // Drift is a constant slope in transit time, so successive differences
        // are constant and small. The estimator must see that as low jitter;
        // correcting drift is drift.rs's job, not this module's.
        let mut buffer = buffer();
        let skew_nanos_per_packet = 60_u64; // 10 ppm at 6 ms per packet

        for n in 0..200_u16 {
            let arrival = u64::from(n) * PACKET_NANOS + u64::from(n) * skew_nanos_per_packet;
            buffer.push(n, u32::from(n) * PACKET_FRAMES, arrival, pcm(0));
            let _ = buffer.pop();
        }

        assert!(
            buffer.jitter_ms() < 0.5,
            "steady skew should not look like jitter, got {} ms",
            buffer.jitter_ms()
        );
    }

    #[test]
    fn reset_clears_everything() {
        let mut buffer = buffer();
        for n in 0..4 {
            push_on_time(&mut buffer, n);
        }
        buffer.reset();

        assert_eq!(buffer.depth_packets(), 0);
        assert_eq!(buffer.pop(), PopOutcome::Starved);
        // A brand new stream numbered from zero must be accepted, not treated
        // as 65k packets late.
        assert_eq!(push_on_time(&mut buffer, 0), PushOutcome::Accepted);
    }

    #[test]
    fn a_total_sender_stall_starves_rather_than_spinning_on_loss() {
        let mut buffer = buffer();
        push_on_time(&mut buffer, 0);
        push_on_time(&mut buffer, 1);
        let _ = drain(&mut buffer);

        for _ in 0..10 {
            assert_eq!(buffer.pop(), PopOutcome::Starved);
        }
        assert_eq!(buffer.stats().lost, 0, "silence is not packet loss");
    }

    // ---- Regressions -----------------------------------------------------
    //
    // Each of these fails against the implementation as it was before the fix
    // it names. They exist because the original suite passed while the buffer
    // was destroying audio.

    #[test]
    fn reordering_before_playback_starts_is_repaired_not_discarded() {
        // The first packet to ARRIVE was latching the playback position, so a
        // lower sequence arriving afterwards was rejected as TooLate even
        // though nothing had been played. Arrival order 2,0,1,3,4,5 lost two
        // packets and still reported zero loss.
        let mut buffer = buffer();
        for n in [2_u16, 0, 1, 3, 4, 5] {
            assert_eq!(
                push_on_time(&mut buffer, n),
                PushOutcome::Accepted,
                "packet {n} must be accepted while the buffer is still filling"
            );
        }

        let popped = drain(&mut buffer);
        assert_eq!(
            popped,
            vec![
                PopOutcome::Packet(pcm(0)),
                PopOutcome::Packet(pcm(1)),
                PopOutcome::Packet(pcm(2)),
                PopOutcome::Packet(pcm(3)),
                PopOutcome::Packet(pcm(4)),
                PopOutcome::Packet(pcm(5)),
            ],
            "every packet must come out, in sequence order"
        );
        assert_eq!(buffer.stats().too_late, 0);
        assert_eq!(buffer.stats().lost, 0);
    }

    #[test]
    fn a_stream_that_does_not_start_at_zero_still_plays_from_its_lowest() {
        let mut buffer = buffer();
        for n in [900_u16, 898, 899, 901] {
            assert_eq!(push_on_time(&mut buffer, n), PushOutcome::Accepted);
        }

        let popped = drain(&mut buffer);
        assert_eq!(popped.len(), 4);
        assert_eq!(popped[0], PopOutcome::Packet(pcm(898_u16 as u8)));
        assert_eq!(buffer.stats().lost, 0);
    }

    #[test]
    fn a_rejected_packet_cannot_move_the_jitter_estimate() {
        // observe_arrival ran before the accept decision, so one stale
        // datagram pinned the target at max_ms for hundreds of packets.
        let mut buffer = buffer();
        for n in 0..30_u16 {
            push_on_time(&mut buffer, n);
            let _ = buffer.pop();
        }

        let jitter_before = buffer.jitter_ms();
        let target_before = buffer.target_ms();

        // A packet whose slot has already played, arriving wildly late.
        assert_eq!(
            buffer.push(1, PACKET_FRAMES, 900_000_000, pcm(1)),
            PushOutcome::TooLate
        );

        assert_eq!(buffer.jitter_ms(), jitter_before, "rejected packet moved J");
        assert_eq!(buffer.target_ms(), target_before);
    }

    #[test]
    fn a_duplicate_cannot_move_the_jitter_estimate() {
        let mut buffer = buffer();
        for n in 0..10_u16 {
            push_on_time(&mut buffer, n);
        }
        let jitter_before = buffer.jitter_ms();

        // Same sequence, absurd arrival time.
        assert_eq!(
            buffer.push(5, 5 * PACKET_FRAMES, 5_000_000_000, pcm(5)),
            PushOutcome::Duplicate
        );
        assert_eq!(buffer.jitter_ms(), jitter_before);
    }

    #[test]
    fn the_sender_timestamp_may_wrap_without_wrecking_the_estimate() {
        // timestamp_frames is a u32 frame counter, so it wraps about every
        // 24.8 hours at 48 kHz. Differencing it as a plain integer turned that
        // into a four-billion-frame jump and pinned the target at max_ms.
        let mut buffer = buffer();
        let start = u32::MAX - 3 * PACKET_FRAMES;

        for n in 0..40_u64 {
            let timestamp = start.wrapping_add((n as u32).wrapping_mul(PACKET_FRAMES));
            let sequence = n as u16;
            buffer.push(sequence, timestamp, n * PACKET_NANOS, pcm(0));
            let _ = buffer.pop();
        }

        assert!(
            buffer.jitter_ms() < 1.0,
            "a timestamp wrap should not register as jitter, got {} ms",
            buffer.jitter_ms()
        );
        assert!(
            buffer.target_ms() < 50.0,
            "target should not be pinned high"
        );
    }

    #[test]
    fn the_jitter_gain_is_the_one_rfc_3550_specifies() {
        // The suite passed with the gain set to 0.5, 0.9 or 1.0, so nothing
        // actually pinned it. One step of the filter from a known state does.
        let mut buffer = buffer();

        // Two packets establish a baseline transit; the second is displaced by
        // a known amount, so |D| is exactly that displacement.
        buffer.push(0, 0, 0, pcm(0));
        let displacement_ms = 16.0;
        buffer.push(
            1,
            PACKET_FRAMES,
            PACKET_NANOS + (displacement_ms * 1_000_000.0) as u64,
            pcm(1),
        );

        // J starts at 0, so after one step J == |D| * gain == |D| / 16.
        let expected = displacement_ms * (1.0 / 16.0);
        assert!(
            (buffer.jitter_ms() - expected).abs() < 0.05,
            "expected {expected} ms after one step at gain 1/16, got {}",
            buffer.jitter_ms()
        );
    }

    #[test]
    fn a_reversed_depth_range_does_not_panic() {
        // JitterConfig is a public struct with no validation, and f64::clamp
        // panics when min > max.
        let buffer = JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                target_ms: 30,
                min_ms: 100,
                max_ms: 50,
                jitter_multiplier: 3.0,
                max_packets: 64,
            },
        );
        let target = buffer.target_ms();
        assert!(target.is_finite(), "target must stay a real number");
        assert!((50.0..=100.0).contains(&target), "got {target}");
    }
}
