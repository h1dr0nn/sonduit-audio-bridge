//! Pacing the hand-off from the jitter buffer into the audio queue.
//!
//! # The two buffers
//!
//! Audio crosses two of them. The jitter buffer absorbs the network and
//! decides *when* a packet is due; the [`crate::handoff`] ring absorbs the gap
//! between the receive thread and the audio callback. The receive thread is
//! the only thing that moves audio from the first into the second, and how
//! much it moves per arriving packet is what this module decides.
//!
//! # Why a rate and not a maximum
//!
//! Draining as much as the jitter buffer will give empties it. An empty jitter
//! buffer stops playing, refills to its target and then releases the lot in a
//! burst, so the latency swings between roughly nothing and the full target on
//! a cycle, and the starve at the bottom of each cycle feeds concealment into
//! audio that arrived perfectly intact. That is audible as crackle, and it is
//! why the allowance is anchored at one packet out per packet in.
//!
//! # Why one-in-one-out is not enough on its own
//!
//! One in and one out stops the queue growing. It does not bring it back down:
//! whatever depth the queue reached, it keeps, because there is no arrival at
//! which fewer than one packet is handed over. A burst at startup -- a socket
//! backlog delivered at wire speed while the audio device is still opening --
//! therefore became the queue's depth for the rest of the session. A device
//! was measured holding a steady 110 ms there against a 36 ms jitter target,
//! with nothing reporting it and nothing shedding it.
//!
//! So the allowance is zero while the queue is more than one packet above its
//! floor. Nothing is dropped and nothing stalls: the callback keeps draining
//! the queue at the device's rate, and the audio that would have been handed
//! over waits one arrival longer in the jitter buffer instead.
//!
//! # What this does not do
//!
//! **It does not reduce total latency, and it cannot.** The audio still
//! exists; it moves from one buffer to the other and the sum is unchanged.
//! Only two things shed a backlog: dropping it, which clicks, and playing it
//! slightly faster than it arrives, which is the resampler's job. What this
//! buys is that the surplus ends up in the buffer that is measured, reported
//! and regulated -- [`crate::ratio::RatioController`] is driven from buffer
//! depth, and audio parked in the queue was invisible to it -- rather than in
//! a ring that nothing watches.

/// Tuning for [`drain_allowance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacingConfig {
    /// Packets the audio queue should hold.
    ///
    /// It covers one callback and the jitter in getting to it, and it is
    /// latency and nothing else, so it is the smallest depth the device can be
    /// relied on to survive.
    pub floor_packets: usize,

    /// Ceiling on packets moved per packet received.
    ///
    /// Above one so a queue that has fallen behind can catch up, and small
    /// enough that catching up cannot starve the socket.
    pub max_per_packet: usize,
}

/// How many packets to move from the jitter buffer into the audio queue for
/// the packet that has just arrived.
///
/// `queued_ms` is what the queue already holds and `packet_ms` is one packet's
/// duration. The result is a rate, not a target depth: the caller hands over
/// this many packets and no more, whatever the jitter buffer is willing to
/// give.
///
/// Three bands, one packet wide each:
///
/// - below the floor, one packet plus enough to make the shortfall up,
///   bounded by [`PacingConfig::max_per_packet`];
/// - within one packet of the floor, exactly one, so a queue that is where it
///   should be is left alone;
/// - more than one packet above the floor, none, so the queue converges back
///   down at the rate the device drains it.
///
/// The middle band is one packet wide because a packet is the granularity the
/// hand-off has. Anything narrower would alternate between two allowances
/// forever without changing the depth it was alternating about.
#[must_use]
pub fn drain_allowance(queued_ms: f64, packet_ms: f64, config: PacingConfig) -> usize {
    // A degenerate format or a queue that has reported something impossible.
    // One packet out per packet in is the neutral answer: it neither grows the
    // queue nor starves it, and it is what every band below settles on.
    if !queued_ms.is_finite() || !packet_ms.is_finite() || packet_ms <= 0.0 {
        return 1;
    }

    let ceiling = config.max_per_packet.max(1);
    let floor = config.floor_packets as f64;
    let level = (queued_ms / packet_ms).max(0.0);

    // Above the band. Hand over nothing and let the callback bring it down.
    if level >= floor + 1.0 {
        return 0;
    }

    // In the band. This is the steady state and it is deliberately identical
    // to the behaviour that fixed the crackle.
    if level >= floor {
        return 1;
    }

    // Below the band. `ceil` rather than `round`, so a queue a fraction of a
    // packet short still gets the packet it is short of.
    let short = (floor - level).ceil().max(0.0) as usize;
    (1 + short).min(ceiling)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKET_MS: f64 = 6.0;

    const fn config() -> PacingConfig {
        PacingConfig {
            floor_packets: 2,
            max_per_packet: 3,
        }
    }

    fn floor_ms() -> f64 {
        config().floor_packets as f64 * PACKET_MS
    }

    /// One arrival on a synthetic timeline.
    ///
    /// The receive thread hands over whatever the allowance says, and the
    /// audio callback takes `consumed_ms` out over the same interval. Nothing
    /// here reads a clock; the interval *is* one packet.
    fn step(queued_ms: f64, consumed_ms: f64) -> f64 {
        let allowance = drain_allowance(queued_ms, PACKET_MS, config());
        assert!(
            allowance <= config().max_per_packet,
            "allowance {allowance}"
        );
        (queued_ms + allowance as f64 * PACKET_MS - consumed_ms).max(0.0)
    }

    #[test]
    fn a_deep_queue_converges_to_the_floor() {
        // The measured device: 110 ms held against a 12 ms floor, steady for
        // four minutes because one in and one out kept it exactly where the
        // startup burst had left it.
        let mut queued = 110.0;
        let mut packets = 0;

        while queued >= floor_ms() + PACKET_MS {
            queued = step(queued, PACKET_MS);
            packets += 1;
            assert!(packets < 100, "still at {queued:.1} ms after {packets}");
        }

        assert!(
            packets <= 20,
            "took {packets} packets, which is {:.0} ms of drift the user hears",
            packets as f64 * PACKET_MS
        );
        assert!(
            queued >= floor_ms(),
            "converged past the floor to {queued:.1} ms, which underruns"
        );
    }

    #[test]
    fn a_queue_at_the_floor_stays_there_rather_than_oscillating() {
        // The failure this must not reintroduce is a queue that empties and
        // refills on a cycle. At the floor the allowance is one, forever.
        let mut queued = floor_ms();

        for packet in 0..1_000 {
            assert_eq!(
                drain_allowance(queued, PACKET_MS, config()),
                1,
                "packet {packet} at {queued:.1} ms"
            );
            queued = step(queued, PACKET_MS);
            assert!(
                (queued - floor_ms()).abs() < f64::EPSILON,
                "moved to {queued:.1} ms on packet {packet}"
            );
        }
    }

    #[test]
    fn convergence_never_drives_the_queue_below_the_floor() {
        // Handing over nothing costs the queue exactly what the device took,
        // so a queue one packet above the floor lands on the floor and not
        // under it. Under it is an underrun, which is the audible half of the
        // failure this is meant to avoid.
        let mut queued = 200.0;

        for _ in 0..200 {
            let before = queued;
            queued = step(queued, PACKET_MS);
            assert!(
                queued >= floor_ms() - f64::EPSILON,
                "fell from {before:.1} ms to {queued:.1} ms, below the {:.1} ms floor",
                floor_ms()
            );
        }
    }

    #[test]
    fn an_empty_queue_fills_to_the_floor_and_stops() {
        let mut queued = 0.0;
        for _ in 0..50 {
            queued = step(queued, PACKET_MS);
        }
        assert!(
            queued >= floor_ms() && queued < floor_ms() + PACKET_MS,
            "settled at {queued:.1} ms, outside the band"
        );
    }

    #[test]
    fn the_rule_inside_the_band_is_the_one_that_was_tested_on_the_device() {
        // The rule that stopped the crackle was `1 + floor - held`, capped.
        // This change is strictly one-sided: it only ever hands over *less*,
        // and only above the band. Everything at or below the floor is
        // byte-for-byte the behaviour that was tested on the device.
        let config = config();
        for tenths in 0..((config.floor_packets as f64 + 1.0) * PACKET_MS * 10.0) as u32 {
            let queued = f64::from(tenths) / 10.0;
            let held = (queued / PACKET_MS).floor() as usize;
            let legacy = (1 + config.floor_packets.saturating_sub(held)).min(config.max_per_packet);
            assert_eq!(
                drain_allowance(queued, PACKET_MS, config),
                legacy,
                "changed behaviour at {queued:.1} ms"
            );
        }
    }

    #[test]
    fn a_bursty_callback_is_survived_without_the_queue_emptying() {
        // A device that takes two packets every other callback. The queue has
        // to swing by that much -- no pacing can prevent it -- but it must
        // stay bounded and must not run dry.
        let mut queued = floor_ms();
        let mut lowest = f64::MAX;
        let mut highest: f64 = 0.0;

        for packet in 0..500 {
            let consumed = if packet % 2 == 0 {
                0.0
            } else {
                2.0 * PACKET_MS
            };
            queued = step(queued, consumed);
            lowest = lowest.min(queued);
            highest = highest.max(queued);
        }

        assert!(lowest > 0.0, "the queue ran dry at {lowest:.1} ms");
        assert!(
            highest <= floor_ms() + 2.0 * PACKET_MS,
            "the queue grew to {highest:.1} ms"
        );
    }

    #[test]
    fn a_sender_running_fast_is_not_absorbed_by_the_queue_forever() {
        // Drift the resampler has not corrected yet: slightly more audio
        // arrives than the device takes. The queue must not quietly swallow
        // it, because the depth that is regulated and reported is upstream of
        // here.
        let mut queued = floor_ms();
        let mut highest: f64 = 0.0;
        for _ in 0..2_000 {
            queued = step(queued, PACKET_MS * 0.99);
            highest = highest.max(queued);
        }
        // One in and one out on its own would have taken the whole 120 ms.
        // The tolerance is for the accumulated addition, not for the bound:
        // the band is one packet wide and the queue stays inside it.
        assert!(
            highest <= floor_ms() + PACKET_MS + 0.001,
            "the queue absorbed the drift and reached {highest:.1} ms"
        );
    }

    #[test]
    fn a_degenerate_packet_length_hands_over_one_rather_than_dividing_by_it() {
        assert_eq!(drain_allowance(12.0, 0.0, config()), 1);
        assert_eq!(drain_allowance(12.0, f64::NAN, config()), 1);
        assert_eq!(drain_allowance(f64::NAN, PACKET_MS, config()), 1);
    }

    #[test]
    fn a_zero_floor_still_hands_over_one_packet_per_packet() {
        // A caller that wants no queue at all must not be read as a caller
        // that wants no audio: the callback still has to be fed.
        let config = PacingConfig {
            floor_packets: 0,
            max_per_packet: 3,
        };
        assert_eq!(drain_allowance(0.0, PACKET_MS, config), 1);
        assert_eq!(drain_allowance(PACKET_MS, PACKET_MS, config), 0);
    }
}
