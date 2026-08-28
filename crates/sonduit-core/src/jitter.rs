//! Adaptive jitter buffer.
//!
//! Absorbs the difference between when packets arrive and when the audio
//! callback needs them. It reorders, detects loss and duplicates, and sizes
//! itself from the inter-arrival jitter estimator in RFC 3550 section 6.4.1.
//!
//! Nothing here reads a clock. Arrival times are passed in, which is what
//! makes the whole module testable against a synthetic timeline.
//!
//! # It discards audio, and always from the front
//!
//! Absorbing is the job, but only up to a point: audio held is latency, and a
//! buffer with no way of giving depth back keeps whatever a bad moment put in
//! it for the rest of the session. So the front of the buffer is discarded
//! when it arrived faster than it was produced -- see [`JitterBuffer::push`],
//! which is the socket backlog a slow device open leaves behind -- when the
//! receiver as a whole is holding past its configured ceiling, which is
//! [`JitterBuffer::shed_over_budget`], and when the packet ceiling in
//! [`JitterConfig::max_packets`] is reached, which is the floor underneath
//! the other two and which a session inside its budget never touches.
//!
//! Always from the front, and never the packet that has just arrived. The
//! oldest audio held is the audio furthest from being playable in time; the
//! newest is the only part of the stream that is still current. A buffer that
//! refused new audio in order to keep old would preserve exactly the wrong
//! second of the stream, and would preserve it for the rest of the session,
//! because nothing else here ever gives depth back. That is not hypothetical:
//! it is what a real device was found doing, pinned at 1536 ms -- 256 packets
//! of six -- with the drift controller at its 500 ppm limit behind it.
//!
//! None of the three is routine and none is silent: all are counted in
//! [`JitterStats::shed`], and all stop at the target rather than emptying the
//! buffer, because a buffer shed below the depth it starts playing at re-arms,
//! and re-arming is audible.

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

/// How much faster than its own duration a packet has to arrive before it is
/// read as backlog rather than as jitter.
///
/// Four means a six-millisecond packet must turn up within a millisecond and a
/// half of the one before it. Ordinary jitter does not reach that, and the
/// bursts Wi-Fi aggregation produces do not sustain it; a socket buffer being
/// emptied at wire speed does nothing else. The comparison is against the
/// packet's *own* timestamp delta, so it holds at any packet size or rate
/// without a second constant to keep in step.
const WIRE_SPEED_RATIO: i64 = 4;

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
    /// Packets held before the buffer sheds its oldest to make room,
    /// protecting against a sender that floods faster than the sink drains.
    ///
    /// A bound on memory and a last resort, not the depth policy. [`max_ms`]
    /// is what is meant to keep the depth sane, but that one is applied by the
    /// caller on every arrival through [`JitterBuffer::shed_over_budget`], and
    /// a caller that does not call it -- or calls it only on some arrivals --
    /// leaves this as the one limit the buffer enforces on itself. Reaching it
    /// means the bound above was not applied, or did not catch what put the
    /// depth here.
    ///
    /// Honoured even when it sits below the adaptive target: the ceiling wins,
    /// and the buffer holds less than it would like to.
    ///
    /// [`max_ms`]: JitterConfig::max_ms
    pub max_packets: usize,

    /// How much better conditions must look before the target is allowed to
    /// shrink, as a ratio of the current target to what jitter now suggests.
    ///
    /// Growing and shrinking on the same threshold makes the target oscillate:
    /// it shrinks the moment the link looks calm, underruns on the next burst,
    /// grows again, and repeats. roc uses 1.7 for the same reason; anything at
    /// or below 1.0 disables the hysteresis entirely.
    pub shrink_threshold: f64,

    /// Packets to wait after shrinking before changing the target again.
    ///
    /// A shrink is the change that can cause an underrun, so it is followed by
    /// the shorter of the two cooldowns: if it was wrong, the buffer needs to
    /// be allowed to grow back quickly.
    pub shrink_cooldown_packets: u32,

    /// Packets to wait after growing before changing the target again.
    ///
    /// Longer than the shrink cooldown. Growing costs latency the user hears,
    /// and giving it straight back at the first quiet moment is what produces
    /// the oscillation this exists to prevent.
    pub grow_cooldown_packets: u32,
}

/// Which link the audio is arriving over.
///
/// The buffer depth that is right for one is wrong for the other, and by
/// enough to matter: ADR-004 puts a single shared constant at either eighteen
/// wasted milliseconds on USB or regular underruns on Wi-Fi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A wireless network. Shared medium, retransmission, and a scheduler that
    /// can stall a station for tens of milliseconds without warning.
    WiFi,
    /// USB tethering. A dedicated wire with no contention and no retry.
    Usb,
}

/// Packets in a row that must declare a different link before it is believed.
///
/// The sender re-declares the link in the header of every packet, so the
/// information is not scarce and waiting for a few more of them is almost
/// free: at six milliseconds a packet this is thirty milliseconds, set
/// against the several seconds of wrongly sized buffer that reacting late
/// costs.
///
/// What it buys is that one datagram cannot retune a live buffer. Nothing
/// authenticates a packet on this wire, so a single spoofed or corrupted
/// header with the flag flipped would otherwise move the target and throw
/// away the drift history behind it. Sustaining the claim for five
/// consecutive packets means holding the stream at the packet rate, and
/// anyone who can do that can inject audio directly and has no need of this.
pub const LINK_CONFIRMATIONS: u32 = 5;

/// Follows the link the sender declares, and reports a change once it holds.
///
/// A run has to be unbroken. One packet agreeing with the link in force ends
/// any argument for changing it, because a stray header among honest ones is
/// exactly what this exists to reject, and a run interrupted by an honest
/// packet is stray by definition. A real migration is not interrupted: the
/// sender has already changed interface and every packet after it says so.
#[derive(Debug, Clone, Copy)]
pub struct LinkWatch {
    link: Transport,
    /// The link being argued for, and how many packets in a row have said so.
    candidate: Option<(Transport, u32)>,
}

impl LinkWatch {
    /// Start watching, with `link` taken as already in force.
    #[must_use]
    pub const fn new(link: Transport) -> Self {
        Self {
            link,
            candidate: None,
        }
    }

    /// The link currently in force.
    #[must_use]
    pub const fn link(&self) -> Transport {
        self.link
    }

    /// Fold in what one packet declared.
    ///
    /// Returns the new link on the packet that confirms a change, and `None`
    /// on every other packet, so a caller can act exactly once per migration.
    pub fn observe(&mut self, declared: Transport) -> Option<Transport> {
        if declared == self.link {
            self.candidate = None;
            return None;
        }

        let seen = match self.candidate {
            Some((candidate, seen)) if candidate == declared => seen.saturating_add(1),
            _ => 1,
        };

        if seen >= LINK_CONFIRMATIONS {
            self.candidate = None;
            self.link = declared;
            return Some(declared);
        }

        self.candidate = Some((declared, seen));
        None
    }
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self::for_transport(Transport::WiFi)
    }
}

impl JitterConfig {
    /// Depths suited to `transport`.
    ///
    /// Wi-Fi gets the conservative numbers because its worst case is not its
    /// average: a station that loses the medium for 40 ms is normal on a busy
    /// access point, and a buffer sized for the average underruns every time
    /// that happens.
    ///
    /// USB has no contention and no retransmission, so the arrival spacing is
    /// nearly deterministic. Holding 30 ms there is 20 ms of latency bought
    /// against a hazard that does not exist on the link.
    #[must_use]
    pub const fn for_transport(transport: Transport) -> Self {
        match transport {
            Transport::WiFi => Self {
                target_ms: 30,
                min_ms: 10,
                max_ms: 200,
                jitter_multiplier: 3.0,
                max_packets: 256,
                shrink_threshold: 1.7,
                // At 6 ms packets these are roughly 5 and 15 seconds, matching
                // roc. Long enough that a single quiet burst cannot move the
                // target, short enough to follow a link that really changed.
                shrink_cooldown_packets: 830,
                grow_cooldown_packets: 2_500,
            },
            Transport::Usb => Self {
                target_ms: 10,
                min_ms: 6,
                // Still adaptive, and still allowed to grow: a phone that is
                // busy can stall its own USB stack, and the floor matters more
                // than the ceiling.
                max_ms: 80,
                jitter_multiplier: 3.0,
                max_packets: 256,
                shrink_threshold: 1.7,
                // Shorter than on Wi-Fi. USB conditions do not drift the way a
                // shared radio does, so a change that persists this long is
                // real rather than a burst.
                shrink_cooldown_packets: 500,
                grow_cooldown_packets: 1_500,
            },
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
    /// Stored, and [`JitterConfig::max_packets`] forced the oldest audio out
    /// to make room. Carries the packets discarded, which are counted in
    /// [`JitterStats::shed`] and are neither lost nor concealed.
    ///
    /// The discard is always from the front, so the newest audio the buffer
    /// holds always survives. A straggler the network reordered can arrive to
    /// find the buffer at its ceiling, and is then judged with the rest of the
    /// front and may go out with it; that is the same decision applied to it
    /// as to its neighbours, and not a refusal.
    ///
    /// Nothing a healthy session does produces this. It says the ceiling in
    /// [`JitterConfig::max_ms`] was not applied, or did not catch what put the
    /// depth here, so it is worth a line in whatever the caller logs to.
    AcceptedShedding(u64),
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
    /// Times the target depth was raised.
    pub target_grew: u64,
    /// Times the target depth was lowered.
    ///
    /// A session where this keeps pace with `target_grew` is oscillating, and
    /// the user is hearing every cycle of it.
    pub target_shrank: u64,
    /// Times the buffer reached [`JitterConfig::max_packets`] and shed its
    /// oldest audio to make room for what had just arrived.
    ///
    /// Not packets dropped and not packets refused: nothing is turned away
    /// here, and the packets each shed discarded are in `shed`. This counts
    /// the event, because the event means the millisecond budget above it did
    /// not do its job.
    pub overflows: u64,
    /// Slots given up on and concealed.
    pub lost: u64,
    /// Calls that found nothing to play.
    pub starved: u64,
    /// Packets that arrived out of order but early enough to be reordered.
    pub reordered: u64,
    /// Packets discarded from the front: a backlog arrived at wire speed, the
    /// receiver was holding past what the link allows, or the buffer reached
    /// [`JitterConfig::max_packets`].
    ///
    /// Not loss and not concealed, in any of the three cases: `next` moves
    /// with the discard, so a shed slot is never given to the concealer. See
    /// [`JitterBuffer::push`] for the first and third and
    /// [`JitterBuffer::shed_over_budget`] for the second. A handful once, at
    /// the start of a session, is the case this exists for. A count that keeps
    /// climbing means the sender is genuinely producing faster than the device
    /// plays, which is drift and belongs to the resampler.
    pub shed: u64,
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
    /// The depth the buffer is actually aiming for, in milliseconds.
    ///
    /// Held rather than recomputed, because the whole point of the hysteresis
    /// is that the target does not follow the jitter estimate immediately.
    target_ms: f64,
    /// Packets accepted, used as the clock for the cooldowns.
    ///
    /// Packets rather than wall time: this crate has no clock, and a buffer
    /// that has received nothing has no reason to retarget anyway.
    accepted_ticks: u32,
    /// The tick at which the target may next change.
    retarget_allowed_at: u32,
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
            // Clamped, not taken as given. JitterConfig has no validation, so
            // a starting target outside the configured range is possible and
            // would be honoured forever if the link never got worse.
            target_ms: f64::from(config.target_ms).clamp(
                f64::from(config.min_ms.min(config.max_ms)),
                f64::from(config.min_ms.max(config.max_ms)),
            ),
            accepted_ticks: 0,
            retarget_allowed_at: 0,
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
    pub const fn target_ms(&self) -> f64 {
        self.target_ms
    }

    /// The depth the current jitter estimate on its own would suggest.
    ///
    /// What the target was before hysteresis; kept public because it is the
    /// only way to see how far the held target has been allowed to lag.
    #[must_use]
    pub fn suggested_target_ms(&self) -> f64 {
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

    /// Move the held target towards what jitter now suggests, if allowed.
    ///
    /// # Why this is not symmetric
    ///
    /// Growing and shrinking on the same rule makes the target oscillate. The
    /// buffer shrinks the moment the link looks calm, underruns on the next
    /// burst, grows again, and repeats, and the user hears every cycle. So a
    /// shrink has to clear a ratio threshold as well as a cooldown, and the
    /// cooldown after growing is the longer of the two: growing costs latency
    /// once, while giving it back too eagerly costs a dropout every time.
    ///
    /// RFC 3550's own text says its estimator "is not intended to be taken
    /// quantitatively", which is the other half of the reason not to follow it
    /// closely.
    fn retarget(&mut self) {
        let suggested = self.suggested_target_ms();

        // Growing is always allowed to start immediately in one case: a target
        // that has never moved is still the configured default, and waiting a
        // cooldown before the first honest measurement is just a slower start.
        let first_move = self.retarget_allowed_at == 0 && self.accepted_ticks > 0;
        if !first_move && self.accepted_ticks < self.retarget_allowed_at {
            return;
        }

        if suggested > self.target_ms {
            self.target_ms = suggested;
            self.retarget_allowed_at = self
                .accepted_ticks
                .saturating_add(self.config.grow_cooldown_packets);
            self.stats.target_grew += 1;
            return;
        }

        // Shrinking needs the link to look not merely better but much better.
        // A threshold at or below one disables the hysteresis, which is a
        // legitimate choice for a caller that wants the raw estimate.
        let threshold = self.config.shrink_threshold.max(1.0);
        if suggested > 0.0 && self.target_ms / suggested >= threshold {
            self.target_ms = suggested;
            self.retarget_allowed_at = self
                .accepted_ticks
                .saturating_add(self.config.shrink_cooldown_packets);
            self.stats.target_shrank += 1;
        }
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
    ///
    /// Returns whether this packet arrived faster than the sender could have
    /// produced it, which is the same two deltas read a different way and is
    /// why it is answered here rather than measured a second time.
    fn observe_arrival(&mut self, timestamp_frames: u32, arrival_nanos: u64) -> bool {
        let arrival_frames =
            (arrival_nanos as i128 * i128::from(self.format.sample_rate) / 1_000_000_000) as i64;

        let mut wire_speed = false;

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

            // Sender time ran on and receiver time did not. A packet cannot be
            // early against its own clock, so the only way this reads true is
            // that the packet was already waiting somewhere when this thread
            // got round to asking for it.
            //
            // Both deltas have to be forward. A packet the network reordered
            // arrives with time running backwards on one clock or both, and
            // the ratio alone would read a pair of negative deltas as the
            // fastest delivery there has ever been.
            wire_speed = sender_delta > 0
                && arrival_delta >= 0
                && arrival_delta.saturating_mul(WIRE_SPEED_RATIO) < sender_delta;
        }

        self.previous_observation = Some((arrival_frames, timestamp_frames));
        wire_speed
    }

    /// Throw away the oldest audio held, down to the target depth.
    ///
    /// `next` moves with it, so nothing discarded here is later reported lost
    /// or handed to the concealer: this is a deliberate discard and not a gap.
    /// The loop stops at the target and never below it, so the buffer cannot
    /// be emptied from here and the starve-refill cycle that produced the
    /// crackle has no way in.
    fn shed_to_target(&mut self) {
        let target = self.target_packets();
        self.shed_down_to(target);
    }

    /// The same discard, to a depth the caller names.
    ///
    /// Split out for the hard ceiling, which has to be able to shed below the
    /// adaptive target. A `max_packets` smaller than the depth `target_ms`
    /// asks for is a legal configuration -- neither field validates against
    /// the other -- and a shed that always stopped at the target would leave
    /// nothing enforcing the ceiling in that case at all.
    ///
    /// Never below one packet, whatever it is given: a buffer shed empty
    /// re-arms, and re-arming is audible.
    fn shed_down_to(&mut self, floor: usize) {
        let floor = floor.max(1);
        while self.entries.len() > floor {
            let Some((oldest, _)) = self.entries.pop_first() else {
                break;
            };
            // Only when playback has already latched onto a slot. Before that
            // `next` is deliberately unset so that `pop` can start at the
            // lowest sequence still held, which after shedding is the right
            // one for free.
            if let Some(next) = self.next.as_mut() {
                *next = oldest + 1;
            }
            self.stats.shed += 1;
        }
    }

    /// Offer a packet to the buffer.
    ///
    /// `arrival_nanos` is a receiver-side monotonic timestamp. It is only ever
    /// differenced, so its epoch does not matter.
    ///
    /// # It can discard audio, and never the packet it was given
    ///
    /// Two of the three discards in this module happen here, and both take
    /// from the front. A packet that arrived faster than the sender could have
    /// produced it is backlog rather than depth, and while the buffer is over
    /// its target the oldest goes; and reaching
    /// [`JitterConfig::max_packets`] discards down to the target and returns
    /// [`PushOutcome::AcceptedShedding`], because at the ceiling the front of
    /// the buffer is the audio that can no longer be played on time and the
    /// arriving packet is the only part of the stream that is current.
    ///
    /// Both move `next` with them, so a discarded slot is never reported lost
    /// and never reaches the concealer. Both are counted in
    /// [`JitterStats::shed`], and the second is counted again as an event in
    /// [`JitterStats::overflows`]. Neither happens on a session that is
    /// behaving.
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

        // Only packets the buffer actually keeps may move the jitter estimate.
        // Observing rejected ones lets a single stale or duplicate datagram
        // pin the target at max_ms for hundreds of packets.
        let wire_speed = self.observe_arrival(timestamp_frames, arrival_nanos);

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
        self.accepted_ticks = self.accepted_ticks.saturating_add(1);

        // Retargeting is driven from the accept path rather than from pop:
        // the jitter estimate only changes when a packet arrives, so a buffer
        // being drained by a silent sender has nothing new to act on.
        self.retarget();

        // A backlog, not a buffer. Audio arriving faster than it was produced
        // was already sitting somewhere -- on a real session, in the kernel's
        // socket buffer while the receive thread was blocked inside AAudio's
        // open -- and every packet of it is audio that can no longer be played
        // on time. Holding it makes it permanent latency: the depth stays
        // wherever the burst left it, and the only thing that sheds it
        // afterwards is the resampler at 500 ppm, which needs minutes for a
        // tenth of a second.
        //
        // So the burst is not allowed to become depth. Above the target, and
        // only while audio is still arriving faster than real time, the oldest
        // packet goes. Both halves matter:
        //
        // - the rate test alone would shed a Wi-Fi catch-up burst, which is
        //   the buffer doing its job after a stall;
        // - the depth test alone would shed on ordinary jitter, and shedding
        //   is not free.
        //
        // Together they name one condition: more audio than the buffer is for,
        // arriving faster than anything could play it. A link that is merely
        // jittery never satisfies the first and a link that is catching up
        // from a stall is below target and never satisfies the second, so on a
        // session that starts cleanly this does nothing at all.
        if wire_speed {
            self.shed_to_target();
        }

        // The floor underneath both of the discards above, and the only limit
        // this buffer applies without being asked. `max_ms` is what is meant
        // to keep the depth sane, but it is policy the caller applies on every
        // arrival by calling `shed_over_budget`, and a caller that does not --
        // or one that skips it while there is no playback queue to measure
        // against -- has nothing between a stalled sink and unbounded memory
        // but this.
        //
        // What it must not do is what it used to do: refuse the packet that
        // has just arrived. At the ceiling the packets at the front are the
        // ones that can no longer be played on time and the new one is the
        // only part of the stream that is current, so refusing it keeps the
        // stale second and a half and throws away the live audio -- and keeps
        // it permanently, because refusing is not a way back down.
        //
        // So the discard is from the front, down to the target the adaptation
        // settled on, or to the ceiling itself when a caller has set one below
        // that target. `next` moves with it, so none of what goes is reported
        // lost or handed to the concealer.
        let ceiling = self.config.max_packets.max(1);
        if self.entries.len() > ceiling {
            let before = self.stats.shed;
            let floor = self.target_packets().min(ceiling);
            self.shed_down_to(floor);
            self.stats.overflows += 1;
            return PushOutcome::AcceptedShedding(self.stats.shed - before);
        }

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

    /// Adopt a different link's tuning without dropping what is held.
    ///
    /// A session can change link while it is playing: the desktop migrates one
    /// between Wi-Fi and a USB tether and declares the new link in the header
    /// of every packet after it. The format has not changed, so nothing about
    /// the stream is new -- only the policy that was right for it.
    ///
    /// This is why that is not a new [`JitterBuffer`]. Constructing one throws
    /// away the ten to thirty milliseconds this one is holding, and that hole
    /// is precisely the gap a seamless migration exists to avoid. So the
    /// packets stay, the sequence state stays, and `playing` stays: a buffer
    /// that re-arms starves for a target's worth of audio, and re-arming is
    /// audible.
    ///
    /// What does change:
    ///
    /// - **the target is recomputed from the new config, not clamped into
    ///   it.** Clamping alone is the failure this replaces. USB's floor of
    ///   6 ms is a legal depth on Wi-Fi, so a buffer arriving from USB would
    ///   keep it and spend the nine seconds the adaptation needs to grow to
    ///   Wi-Fi's 30 ms holding a wire's worth of audio against a radio's
    ///   jitter -- nine seconds of plausible underrun, beginning the moment
    ///   the user pulled the cable.
    /// - **the jitter estimate is cleared.** It is the mean absolute change in
    ///   transit time on the path that has just been left, and it describes
    ///   nothing about the new one. Clearing it is also what puts the target
    ///   on the new link's configured depth rather than the old link's
    ///   measured one, in both directions.
    /// - **the retarget cooldown is cleared,** so the first arrival over the
    ///   new link may move the target immediately. A cooldown is a promise not
    ///   to react to a link that no longer carries the audio.
    /// - **the previous arrival is forgotten,** so the one packet that
    ///   straddles the change is not differenced against the other path. That
    ///   difference is a step and not jitter; read as jitter it inflates the
    ///   estimate, and read by the wire-speed test it sheds audio on the
    ///   strength of a single crossing.
    ///
    /// [`JitterStats`] and the cooldown clock keep running. The session did
    /// not restart; its link changed.
    pub fn retune(&mut self, config: JitterConfig) {
        self.config = config;
        // Same clamp as `new`: JitterConfig has no validation, and a target
        // outside its own range would otherwise be honoured forever.
        self.target_ms = f64::from(config.target_ms).clamp(
            f64::from(config.min_ms.min(config.max_ms)),
            f64::from(config.min_ms.max(config.max_ms)),
        );
        self.jitter_frames = 0.0;
        self.previous_observation = None;
        self.retarget_allowed_at = 0;
    }

    /// Drop everything and re-arm, for a format change or a new sender.
    pub fn reset(&mut self) {
        self.target_ms = f64::from(self.config.target_ms).clamp(
            f64::from(self.config.min_ms.min(self.config.max_ms)),
            f64::from(self.config.min_ms.max(self.config.max_ms)),
        );
        self.accepted_ticks = 0;
        self.retarget_allowed_at = 0;
        self.entries.clear();
        self.extender = SequenceExtender::default();
        self.next = None;
        self.previous_observation = None;
        self.jitter_frames = 0.0;
        self.playing = false;
    }

    /// Shed the oldest audio while the receiver is holding more than the link
    /// is allowed to hold.
    ///
    /// `downstream_ms` is audio that has already left this buffer and has not
    /// been played yet, which on a real session is the [`crate::handoff`]
    /// ring. It is counted because the listener waits through the sum: which
    /// of the two buffers a millisecond is sitting in does not change how late
    /// it is.
    ///
    /// Returns the packets discarded, which is zero in the ordinary case.
    ///
    /// # Why the bound is `max_ms`
    ///
    /// [`JitterConfig::max_ms`] is already the most this link should ever
    /// hold, and it only ever clamped the adaptive *target*. Nothing bounded
    /// what the buffer actually contained except `max_packets`, which is 256,
    /// and at 6 ms a packet that is 1536 ms -- the depth a real USB session
    /// was measured pinned at, dropping every newly arrived packet to keep a
    /// second and a half of stale audio, with the drift controller at its
    /// 500 ppm limit and fifty minutes of resampling ahead of it.
    ///
    /// So the budget is read as a bound on the total. The ring's occupancy is
    /// spent out of it first, because that audio has already been handed to
    /// the callback and only the callback could give it back, which is a
    /// decision that does not belong at realtime priority. What is left is
    /// this buffer's share, and the share is never less than the target the
    /// adaptation settled on: shedding below the target is what starves
    /// playback, and the starve-refill cycle is the crackle both this and the
    /// pacing rule exist to avoid.
    ///
    /// # Why this is not routine
    ///
    /// It throws audio away and the listener hears the join, so it is the same
    /// last resort as [`crate::handoff::Producer::resync_if_hopeless`] -- the
    /// backstop that used to catch this case by accident, back when the
    /// surplus collected in the ring it watches. It fires only above the
    /// budget, it lands on the target rather than on the budget so that one
    /// shed buys back the whole difference instead of a packet at a time, and
    /// the packet of slack keeps whole-packet granularity from tripping it.
    /// A session inside its budget never reaches any of that.
    pub fn shed_over_budget(&mut self, downstream_ms: f64) -> u64 {
        let packet_ms = self
            .format
            .packet_duration_nanos()
            .map_or(0.0, |nanos| nanos as f64 / 1_000_000.0);
        let budget = f64::from(self.config.max_ms);

        // A caller that has not configured a ceiling is not asking for one,
        // and a degenerate format has no packet to measure the slack in.
        if packet_ms <= 0.0 || budget <= 0.0 || !downstream_ms.is_finite() {
            return 0;
        }

        let share = (budget - downstream_ms.max(0.0)).max(self.target_ms());
        if self.depth_ms() <= share + packet_ms {
            return 0;
        }

        let before = self.stats.shed;
        self.shed_to_target();
        self.stats.shed - before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_holds_less_audio_than_wifi() {
        // The whole reason the two are distinguished. One constant for both
        // either wastes latency on the wire or underruns on the air.
        let wifi = JitterConfig::for_transport(Transport::WiFi);
        let usb = JitterConfig::for_transport(Transport::Usb);

        assert!(
            usb.target_ms < wifi.target_ms,
            "usb {} ms is not below wifi {} ms",
            usb.target_ms,
            wifi.target_ms
        );
        assert!(usb.min_ms < wifi.min_ms);
    }

    #[test]
    fn every_transport_leaves_room_to_adapt_upwards() {
        // A ceiling at or below the target would pin the buffer and defeat the
        // adaptation entirely.
        for transport in [Transport::WiFi, Transport::Usb] {
            let config = JitterConfig::for_transport(transport);
            assert!(
                config.max_ms > config.target_ms,
                "{transport:?} cannot grow past its target"
            );
            assert!(
                config.min_ms < config.target_ms,
                "{transport:?} cannot shrink below its target"
            );
        }
    }

    #[test]
    fn the_default_is_the_conservative_transport() {
        // Anything that has not been told which link it is on is guessing, and
        // guessing wrong towards wifi costs latency while guessing wrong
        // towards usb costs dropouts.
        let default = JitterConfig::default();
        let wifi = JitterConfig::for_transport(Transport::WiFi);
        assert_eq!(default.target_ms, wifi.target_ms);
    }

    #[test]
    fn a_usb_buffer_starts_playing_sooner_than_a_wifi_one() {
        // The configuration only matters if it reaches the buffer, so this
        // asserts on behaviour rather than on the numbers.
        let format = Format::stereo_48k();
        let mut usb = JitterBuffer::new(format, JitterConfig::for_transport(Transport::Usb));
        let mut wifi = JitterBuffer::new(format, JitterConfig::for_transport(Transport::WiFi));

        let payload = crate::format::PCM_PAYLOAD_BYTES;
        let frames = (payload / 4) as u32;
        let mut usb_started = None;
        let mut wifi_started = None;

        for sequence in 0..40_u16 {
            let pcm = vec![1_u8; payload];
            usb.push(sequence, u32::from(sequence) * frames, 0, pcm.clone());
            wifi.push(sequence, u32::from(sequence) * frames, 0, pcm);

            if usb_started.is_none() && !matches!(usb.pop(), PopOutcome::Starved) {
                usb_started = Some(sequence);
            }
            if wifi_started.is_none() && !matches!(wifi.pop(), PopOutcome::Starved) {
                wifi_started = Some(sequence);
            }
        }

        let usb_started = usb_started.expect("usb never started playing");
        let wifi_started = wifi_started.expect("wifi never started playing");
        assert!(
            usb_started < wifi_started,
            "usb started at packet {usb_started}, wifi at {wifi_started}"
        );
    }

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
                ..JitterConfig::default()
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
        // A ceiling below the depth the target asks for, which is a legal
        // configuration: neither field validates against the other. The
        // ceiling has to win there, or nothing bounds this buffer at all.
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

        for n in 4..12 {
            assert_eq!(
                push_on_time(&mut buffer, n),
                PushOutcome::AcceptedShedding(1),
                "packet {n} was not stored at the ceiling"
            );
            assert_eq!(buffer.depth_packets(), 4, "grew past the ceiling at {n}");
        }
        assert_eq!(buffer.stats().overflows, 8);
        assert_eq!(buffer.stats().shed, 8);
    }

    #[test]
    fn at_the_ceiling_the_audio_kept_is_the_recent_audio() {
        // The defect this exists for. At the ceiling the buffer used to refuse
        // the packet that had just arrived, so what it held was the oldest
        // audio it had and the current audio was thrown away -- and it stayed
        // that way, because refusing is not a way back down.
        let ceiling = 8;
        let sent = 40_u16;
        let mut buffer = JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                max_packets: ceiling,
                ..JitterConfig::default()
            },
        );
        for n in 0..sent {
            push_on_time(&mut buffer, n);
        }

        let mut played = Vec::new();
        while let PopOutcome::Packet(pcm) = buffer.pop() {
            played.push(u16::from(pcm[0]));
        }

        assert!(!played.is_empty(), "the buffer played nothing");
        assert!(
            played.len() <= ceiling,
            "held {} past the ceiling",
            played.len()
        );

        // What came out is the end of what went in, not the beginning.
        let expected: Vec<u16> = (sent - played.len() as u16..sent).collect();
        assert_eq!(
            played, expected,
            "the buffer played the stale audio, not the recent audio"
        );
        assert_eq!(
            played.last(),
            Some(&(sent - 1)),
            "the newest packet was lost"
        );

        // Everything sent is accounted for, and every packet that did not play
        // was shed rather than refused or concealed.
        let stats = buffer.stats();
        assert_eq!(stats.accepted, u64::from(sent));
        assert_eq!(stats.shed as usize + played.len(), usize::from(sent));
        assert_eq!(stats.lost, 0);
    }

    #[test]
    fn nothing_shed_at_the_ceiling_is_reported_lost() {
        let mut buffer = JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                max_packets: 8,
                ..JitterConfig::default()
            },
        );
        // Playing first, so `next` is latched and a discard from the front has
        // something it could get wrong.
        for n in 0..8 {
            push_on_time(&mut buffer, n);
        }
        assert!(matches!(buffer.pop(), PopOutcome::Packet(_)));

        for n in 8..60 {
            push_on_time(&mut buffer, n);
        }
        loop {
            match buffer.pop() {
                PopOutcome::Packet(_) => {}
                PopOutcome::Lost => panic!("a shed slot was handed to the concealer"),
                PopOutcome::Starved => break,
            }
        }

        let stats = buffer.stats();
        assert!(stats.shed > 0, "the ceiling was never reached");
        assert_eq!(
            stats.lost, 0,
            "{} shed slots were reported lost",
            stats.lost
        );
        assert_eq!(stats.too_late, 0);
    }

    #[test]
    fn a_buffer_nowhere_near_the_ceiling_is_untouched() {
        let mut buffer = buffer();
        for n in 0..8 {
            assert_eq!(push_on_time(&mut buffer, n), PushOutcome::Accepted);
        }
        let stats = buffer.stats();
        assert_eq!(stats.overflows, 0);
        assert_eq!(stats.shed, 0);
        assert_eq!(buffer.depth_packets(), 8);
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
            ..JitterConfig::default()
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

        for n in 0..60_u16 {
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
                ..JitterConfig::default()
            },
        );
        // Both the held target and the one the estimate suggests have to stay
        // inside the range, however the range was written.
        for target in [buffer.target_ms(), buffer.suggested_target_ms()] {
            assert!(target.is_finite(), "target must stay a real number");
            assert!((50.0..=100.0).contains(&target), "got {target}");
        }
    }

    /// Push packet `n` as arriving `arrival` nanoseconds into the session,
    /// whatever its timestamp says it should have been.
    fn push_at(buffer: &mut JitterBuffer, n: u16, arrival: u64) -> PushOutcome {
        buffer.push(n, u32::from(n) * PACKET_FRAMES, arrival, pcm(n as u8))
    }

    /// The measured session: one packet on time, then the socket buffer
    /// emptying at wire speed while nothing pops, because on the device
    /// nothing could -- the audio callback had not started.
    fn after_a_startup_burst(packets: u16) -> JitterBuffer {
        let mut buffer = buffer();
        push_on_time(&mut buffer, 0);
        for n in 1..packets {
            // Twenty microseconds apart, carrying six milliseconds each.
            push_at(&mut buffer, n, PACKET_NANOS + u64::from(n) * 20_000);
        }
        buffer
    }

    #[test]
    fn a_backlog_arriving_at_wire_speed_does_not_become_the_depth() {
        let buffer = after_a_startup_burst(19);

        // Eighteen packets of backlog, and what is left is what the buffer is
        // for. Before this the whole hundred and eight milliseconds stayed,
        // and one packet in for one packet out meant it stayed for good.
        assert!(
            buffer.depth_ms() <= buffer.target_ms() + 6.0,
            "held {:.1} ms against a {:.1} ms target",
            buffer.depth_ms(),
            buffer.target_ms()
        );
        assert_eq!(
            buffer.depth_packets() + buffer.stats().shed as usize,
            19,
            "packets went missing that were neither held nor shed"
        );
    }

    #[test]
    fn a_backlog_is_shed_from_the_front_and_not_reported_as_loss() {
        let mut buffer = after_a_startup_burst(19);
        let popped = drain(&mut buffer);

        // What survives is the newest audio, contiguous. Shedding the front is
        // what makes the join a single step forward through the stream rather
        // than a hole in the middle of it.
        let tags: Vec<u8> = popped
            .iter()
            .filter_map(|outcome| match outcome {
                PopOutcome::Packet(pcm) => Some(pcm[0]),
                _ => None,
            })
            .collect();
        assert!(!tags.is_empty(), "nothing was left to play");
        assert_eq!(tags.last(), Some(&18), "the newest audio was not kept");
        for pair in tags.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "a hole was left at {}", pair[0]);
        }

        // And none of it is concealed. Audio deliberately discarded is not
        // audio that failed to arrive, so nothing here should be asking the
        // concealer to invent a replacement for it.
        assert_eq!(buffer.stats().lost, 0, "shed packets were reported lost");
        assert!(
            !popped.contains(&PopOutcome::Lost),
            "the shed packets were handed to concealment"
        );
    }

    #[test]
    fn shedding_leaves_enough_to_start_playing() {
        // The failure this must not reintroduce is a buffer that empties,
        // re-arms and then releases the lot in a burst. Shedding stops at the
        // target, so playback starts on the next pop rather than waiting to
        // refill.
        let mut buffer = after_a_startup_burst(19);
        assert!(
            matches!(buffer.pop(), PopOutcome::Packet(_)),
            "the buffer was shed below the depth it starts playing at"
        );
    }

    #[test]
    fn a_link_that_is_merely_jittery_is_not_read_as_a_backlog() {
        // Two milliseconds either side of a six millisecond spacing, which is
        // heavy jitter for a wire and unremarkable for a radio. Nothing pops,
        // so the depth is far past the target the whole time: if the rate test
        // were not doing the work here, this would shed on every packet.
        let mut buffer = buffer();
        let mut arrival = 0;
        for n in 0..60_u16 {
            push_at(&mut buffer, n, arrival);
            arrival += if n % 2 == 0 { 4_000_000 } else { 8_000_000 };
        }

        assert_eq!(
            buffer.stats().shed,
            0,
            "shed audio from a link that was only jittery"
        );
        assert_eq!(buffer.depth_packets(), 60, "packets went missing");
    }

    #[test]
    fn a_reordered_packet_is_not_read_as_a_backlog() {
        // Arriving out of order runs time backwards on both clocks at once,
        // and a ratio test that did not insist on forward deltas would read
        // that pair of negatives as the fastest delivery there has ever been.
        let mut buffer = buffer();
        for n in [0_u16, 2, 1, 4, 3, 6, 5] {
            push_on_time(&mut buffer, n);
        }

        assert_eq!(buffer.stats().shed, 0, "reordering was read as a backlog");
        assert_eq!(buffer.depth_packets(), 7);
    }

    #[test]
    fn a_receiver_inside_its_budget_is_left_alone() {
        // Ten packets is sixty milliseconds against a two hundred millisecond
        // ceiling. Shedding here would be throwing audio away to fix nothing.
        let mut buffer = buffer();
        for n in 0..10 {
            push_on_time(&mut buffer, n);
        }

        assert_eq!(buffer.shed_over_budget(0.0), 0);
        assert_eq!(buffer.stats().shed, 0);
        assert_eq!(buffer.depth_packets(), 10);
    }

    #[test]
    fn a_receiver_past_its_budget_comes_back_to_the_target() {
        // Forty packets is two hundred and forty milliseconds, past the two
        // hundred this link is allowed. Nothing else in the receiver bounds
        // it: the pacing rule holds the ring near its floor, so the surplus
        // collects here and the ring's own backstop never sees it.
        let mut buffer = buffer();
        for n in 0..40 {
            push_on_time(&mut buffer, n);
        }

        let shed = buffer.shed_over_budget(0.0);
        assert!(shed > 0, "nothing was shed from a buffer over its budget");
        assert!(
            buffer.depth_ms() <= buffer.target_ms() + 6.0,
            "came back only to {:.1} ms",
            buffer.depth_ms()
        );

        // One step, not a slow bleed: a second call has nothing left to do.
        assert_eq!(buffer.shed_over_budget(0.0), 0);
    }

    #[test]
    fn the_ring_is_spent_out_of_the_budget_before_this_buffer_is() {
        // The same depth, judged twice. Audio already handed to the callback
        // counts against the ceiling exactly as much as audio still held here,
        // because the listener waits through both.
        let mut buffer = buffer();
        for n in 0..30 {
            push_on_time(&mut buffer, n);
        }

        assert_eq!(
            buffer.shed_over_budget(0.0),
            0,
            "shed while the receiver as a whole was inside its budget"
        );
        assert!(
            buffer.shed_over_budget(150.0) > 0,
            "ignored a hundred and fifty milliseconds sitting in the ring"
        );
    }

    #[test]
    fn shedding_to_a_budget_never_goes_below_the_target() {
        // A ring so full that this buffer's share of the budget is negative.
        // The share is floored at the target, because a buffer shed below the
        // depth it starts playing at re-arms, and re-arming is the crackle.
        let mut buffer = buffer();
        for n in 0..40 {
            push_on_time(&mut buffer, n);
        }

        buffer.shed_over_budget(10_000.0);
        assert!(
            buffer.depth_ms() >= buffer.target_ms() - f64::EPSILON,
            "left {:.1} ms against a {:.1} ms target",
            buffer.depth_ms(),
            buffer.target_ms()
        );
        assert!(matches!(buffer.pop(), PopOutcome::Packet(_)));
    }

    #[test]
    fn a_budget_that_cannot_be_read_sheds_nothing() {
        // A caller with no ceiling configured is not asking for one, and a
        // depth reported as not-a-number is not a depth. Neither is a reason
        // to throw audio away.
        let mut unbounded = JitterBuffer::new(
            Format::stereo_48k(),
            JitterConfig {
                target_ms: 12,
                min_ms: 6,
                max_ms: 0,
                jitter_multiplier: 3.0,
                max_packets: 64,
                ..JitterConfig::default()
            },
        );
        for n in 0..40 {
            push_on_time(&mut unbounded, n);
        }
        assert_eq!(unbounded.shed_over_budget(0.0), 0);

        let mut buffer = buffer();
        for n in 0..40 {
            push_on_time(&mut buffer, n);
        }
        assert_eq!(buffer.shed_over_budget(f64::NAN), 0);
        assert_eq!(buffer.shed_over_budget(f64::INFINITY), 0);
    }
}
