//! Latency that goes up on Wi-Fi and does not come back.
//!
//! The complaint these are written against: over a USB tether the figure sits
//! at about 40 ms, and over Wi-Fi it swings between 50 and 200 ms and sounds
//! late. Two separate one-way movements were found behind it, and neither is
//! the one the report guessed at.
//!
//! # The target could not shrink at all
//!
//! `retarget` grows the held target to whatever the estimate suggests and
//! shrinks it only when the target is `shrink_threshold` -- 1.7 -- times that
//! suggestion. The suggestion used to be floored at `JitterConfig::target_ms`,
//! which is 30 ms on Wi-Fi, so no target below 51 ms could ever satisfy the
//! ratio, and every target the estimator actually produces on a real radio
//! lands in that band. Driven over five minutes of jittery arrivals the
//! grow counter reached eighteen and the shrink counter stayed at zero.
//!
//! # The depth walks up to the ceiling
//!
//! The target is not what the listener waits through. `push` sheds a backlog
//! only when it arrives faster than a quarter of real time, so an access point
//! that releases its queue after a stall at anything slower leaves the surplus
//! in the buffer, where the only thing that removes it is `shed_over_budget`
//! at `JitterConfig::max_ms`. That constant was 200 ms, and 200 ms is
//! therefore where a stalling link parks: not a ceiling, an operating point.
//!
//! # Where the arrival patterns come from
//!
//! Not from the reporter's network, which could not be reached while these
//! were written. The delay figures are the ones collected in
//! `docs/research/jitter-and-drift.md`, and the shape -- a station losing the
//! medium for tens of milliseconds and then being handed its backlog -- is
//! what that document describes an access point doing. They are an input to
//! the test, not a measurement, and the property asserted is the buffer's
//! response to them.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::handoff::{self, Consumer, Producer};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, Transport};
use sonduit_core::pacing::{drain_allowance, PacingConfig};

const RATE: u64 = 48_000;
const CHANNELS: usize = 2;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

const WIFI: JitterConfig = JitterConfig::for_transport(Transport::WiFi);

/// The top of the Wi-Fi band in `docs/latency-budget.md`, mouth to ear.
///
/// Everything the receiver holds is two of the nine stages in that band, so a
/// ceiling above this one cannot be inside the budget under any reading of it.
const WIFI_BAND_TOP_MS: u32 = 80;

/// What the receive thread in `sonduit-ffi` is configured with, duplicated
/// because that crate cannot be built for this test host.
const PACING: PacingConfig = PacingConfig {
    floor_packets: 2,
    max_per_packet: 3,
};

fn packet() -> Vec<u8> {
    vec![1_u8; PCM_PAYLOAD_BYTES]
}

/// Push `count` packets from `from`, each delayed by `delay(index)` against
/// the instant its sender produced it.
///
/// Delaying against the sender's clock rather than against the previous
/// arrival is what makes a spike a spike: the packet behind a delayed one is
/// on time again, which is the pair of differences RFC 3550's estimator reads.
fn feed(buffer: &mut JitterBuffer, from: u32, count: u32, delay_nanos: impl Fn(u32) -> u64) -> u32 {
    for index in 0..count {
        let sequence = from + index;
        let sent = u64::from(sequence) * PACKET_NANOS;
        buffer.push(
            sequence as u16,
            sequence.wrapping_mul(FRAMES_PER_PACKET),
            sent + delay_nanos(index),
            packet(),
        );
        // Drained as a player would, so the buffer does not merely fill up.
        let _ = buffer.pop();
    }
    from + count
}

/// One packet in five arrives 30 ms late.
///
/// Chosen for what it does to the estimator rather than for its own sake. The
/// mean absolute change in transit time it produces is 12 ms, which is inside
/// the band `docs/research/jitter-and-drift.md` gives for a loaded access
/// point, and three times that is a target of 42 ms -- above the configured
/// 30 and below 1.7 times it. That is exactly the band a target could not
/// leave, and it is the band an ordinary bad radio puts it in.
fn loaded_radio(index: u32) -> u64 {
    if index % 5 == 0 {
        30_000_000
    } else {
        0
    }
}

/// A link with nothing wrong with it.
fn calm(_: u32) -> u64 {
    0
}

#[test]
fn a_target_grown_on_a_bad_patch_comes_back_down_when_the_link_calms() {
    let mut buffer = JitterBuffer::new(Format::stereo_48k(), WIFI);

    // Long enough to clear the 2500-packet grow cooldown twice, so the target
    // reaches what the link really costs rather than what a half-converged
    // estimate suggested on the first arrival.
    let next = feed(&mut buffer, 0, 8_000, loaded_radio);
    let grown = buffer.target_ms();

    // The premise. Without this the test below would pass on a buffer that
    // never moved at all.
    assert!(
        grown > f64::from(WIFI.target_ms),
        "the loaded-radio pattern never grew the target past {} ms, got {grown}",
        WIFI.target_ms
    );
    assert!(
        grown < f64::from(WIFI.target_ms) * WIFI.shrink_threshold,
        "this pattern is meant to land inside the band a target could not \
         leave, below {} ms, and it reached {grown}",
        f64::from(WIFI.target_ms) * WIFI.shrink_threshold
    );

    // Two minutes of a link with nothing wrong with it, which is twenty times
    // the cooldown that follows a shrink.
    feed(&mut buffer, next, 20_000, calm);

    let stats = buffer.stats();
    assert!(
        stats.target_shrank > 0,
        "the target grew {} times and shrank none: it is a ratchet, and it is \
         still holding {} ms two minutes after the link recovered",
        stats.target_grew,
        buffer.target_ms()
    );
    assert!(
        (buffer.target_ms() - f64::from(WIFI.target_ms)).abs() < 0.01,
        "the target settled at {} ms on a calm link that asks for {} ms",
        buffer.target_ms(),
        WIFI.target_ms
    );
}

#[test]
fn the_first_shrink_is_a_step_towards_the_estimate_and_not_a_jump_to_it() {
    // roc pairs the 1.7 threshold with a decrease of (x + 1) / 2x of the
    // current target, and this project took the threshold without the step.
    // Setting the target straight to the estimate is a change in latency of
    // whatever the difference happens to be, applied in one arrival.
    let mut buffer = JitterBuffer::new(Format::stereo_48k(), WIFI);

    let next = feed(&mut buffer, 0, 8_000, loaded_radio);
    let grown = buffer.target_ms();

    // Just past one shrink cooldown, so exactly one shrink has been allowed.
    let mut sequence = next;
    let mut before = grown;
    while buffer.stats().target_shrank == 0 && sequence < next + 20_000 {
        before = buffer.target_ms();
        sequence = feed(&mut buffer, sequence, 1, calm);
    }

    assert_eq!(buffer.stats().target_shrank, 1, "no shrink happened at all");
    let after = buffer.target_ms();
    assert!(
        after < before,
        "the shrink did not move the target: {before} to {after}"
    );
    assert!(
        after > f64::from(WIFI.target_ms),
        "the first shrink went the whole way to {} ms in one arrival, from \
         {before}",
        WIFI.target_ms
    );
}

/// One receiver, driven exactly as `sonduit-ffi`'s receive loop drives one.
struct Session {
    buffer: JitterBuffer,
    queue: Producer,
    device: Consumer,
    played_to: u64,
    peak_held_ms: f64,
}

impl Session {
    fn new() -> Self {
        let format = Format::stereo_48k();
        let (queue, device) = handoff::channel(format, 400);
        Self {
            buffer: JitterBuffer::new(format, WIFI),
            queue,
            device,
            played_to: 0,
            peak_held_ms: 0.0,
        }
    }

    fn play_until(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.played_to);
        self.played_to = now;
        let frames = (elapsed * RATE / 1_000_000_000) as usize;
        if frames == 0 {
            return;
        }
        let mut out = vec![0_i16; frames * CHANNELS];
        self.device.fill(&mut out, frames);
    }

    fn receive(&mut self, sequence: u32, arrival: u64) {
        self.play_until(arrival);
        self.buffer.push(
            sequence as u16,
            sequence.wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            packet(),
        );
        self.buffer.shed_over_budget(self.queue.queued_ms());

        let allowance = drain_allowance(self.queue.queued_ms(), PACKET_MS, PACING);
        for _ in 0..allowance {
            match self.buffer.pop() {
                PopOutcome::Packet(pcm) => {
                    if self.queue.push(&pcm) < pcm.len() {
                        break;
                    }
                }
                PopOutcome::Lost => {
                    let silence = vec![0_u8; PCM_PAYLOAD_BYTES];
                    if self.queue.push(&silence) < silence.len() {
                        break;
                    }
                }
                PopOutcome::Starved => break,
            }
        }

        let held = self.buffer.depth_ms() + self.queue.queued_ms();
        self.peak_held_ms = self.peak_held_ms.max(held);
    }
}

#[test]
fn a_stalling_access_point_does_not_walk_the_receiver_up_to_its_ceiling() {
    // The measured shape of the complaint. The medium is lost for 120 ms every
    // twelve seconds and the backlog is then handed over at 2 ms a packet,
    // which is three times real time and so looks like a burst to a listener
    // but not to `push`: the wire-speed test wants a quarter of real time, so
    // none of this is shed on the way in and all of it becomes depth.
    //
    // Twenty-five of those stalls used to leave the receiver holding a median
    // of 122 ms and a 95th percentile of 194 ms, against a link whose whole
    // budget in docs/latency-budget.md is 40 to 80 ms.
    let stall_nanos = 120_000_000_u64;
    let stall_every = 12_000_000_000_u64;
    let burst_gap = 2_000_000_u64;

    let mut session = Session::new();
    let mut medium_free_at = 0_u64;
    let packets = 50_000_u32;

    for sequence in 0..packets {
        let sent = u64::from(sequence) * PACKET_NANOS;
        if sequence > 0 && sent / stall_every != (sent - PACKET_NANOS) / stall_every {
            medium_free_at = medium_free_at.max(sent) + stall_nanos;
        }
        let arrival = sent.max(medium_free_at);
        medium_free_at = arrival + burst_gap;
        session.receive(sequence, arrival);
    }

    // The bound is the configured ceiling on everything the receiver holds,
    // plus the packet of granularity a shed cannot see inside.
    let bound = f64::from(WIFI.max_ms) + PACKET_MS;
    assert!(
        session.peak_held_ms <= bound,
        "the receiver walked up to {:.0} ms against a {bound:.0} ms bound",
        session.peak_held_ms
    );

    // And the ceiling itself has to be a figure the product can afford, or the
    // bound above is satisfied by a receiver holding a quarter of a second.
    const {
        assert!(
            WIFI.max_ms <= WIFI_BAND_TOP_MS,
            "a Wi-Fi receiver may hold more than the whole mouth-to-ear band \
             docs/latency-budget.md targets"
        );
    }
}
