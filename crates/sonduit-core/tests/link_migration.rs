//! What happens to a live buffer when the link underneath it changes.
//!
//! The desktop can move a running session between Wi-Fi and a USB tether
//! without stopping it, and declares the link it is on in the header of every
//! packet. The format never changes, so the receiver has nothing else to
//! notice: the flag flips mid-stream and the audio carries on.
//!
//! The depth that is right for one link is wrong for the other. Wi-Fi holds
//! 30 ms because a contended access point can stall a station for tens of
//! milliseconds; USB holds 10 ms with a floor of 6 because a wire does not.
//! A buffer that keeps the old policy after a migration is wrong in one of two
//! ways, and they are not symmetric:
//!
//! - Wi-Fi to USB holds 30 ms where 10 would do. Twenty wasted milliseconds,
//!   given back by the adaptation in a few seconds. Not dangerous.
//! - USB to Wi-Fi holds a wire's floor against a radio's jitter, and the
//!   adaptation needs about nine seconds to grow out of it. That is nine
//!   seconds of plausible underrun starting the moment the user pulls a
//!   cable, which is the moment they are listening hardest.
//!
//! So the policy has to follow the link, and it has to follow it without
//! dropping the audio already in hand -- a buffer rebuilt from scratch loses
//! what it was holding and re-arms, which is the exact gap a seamless
//! migration exists to avoid. Every timeline below is synthetic: this crate
//! has no clock.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::jitter::{
    JitterBuffer, JitterConfig, JitterStats, LinkWatch, PopOutcome, Transport, LINK_CONFIRMATIONS,
};

const RATE: u64 = 48_000;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

const WIFI: JitterConfig = JitterConfig::for_transport(Transport::WiFi);
const USB: JitterConfig = JitterConfig::for_transport(Transport::Usb);

fn buffer(config: JitterConfig) -> JitterBuffer {
    JitterBuffer::new(Format::stereo_48k(), config)
}

/// A packet whose payload names its own sequence, so the drain below can prove
/// it got the audio it was holding rather than merely the right amount of it.
fn packet(sequence: u16) -> Vec<u8> {
    vec![sequence as u8; PCM_PAYLOAD_BYTES]
}

/// Push `count` packets from `from`, each `spacing(index)` after the last.
///
/// Returns the sequence and arrival time reached, so a session can continue
/// with a different pattern.
fn feed(
    buffer: &mut JitterBuffer,
    from: u16,
    count: u16,
    mut arrival: u64,
    spacing: impl Fn(u16) -> u64,
    drain: bool,
) -> (u16, u64) {
    let mut sequence = from;
    for index in 0..count {
        arrival += spacing(index);
        buffer.push(
            sequence,
            u32::from(sequence).wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            packet(sequence),
        );
        sequence = sequence.wrapping_add(1);
        if drain {
            let _ = buffer.pop();
        }
    }
    (sequence, arrival)
}

/// Perfectly even arrivals, which is what a wire looks like.
fn smooth(_: u16) -> u64 {
    PACKET_NANOS
}

/// Arrivals that bunch and then stall, which is what a contended access point
/// does to a station: several packets land together, then nothing for 60 ms
/// while the medium is busy.
///
/// Deliberately extreme, and the size of the stall is load bearing. The
/// RFC 3550 estimator converges on the mean absolute change in transit time,
/// so this pattern settles it near 33 ms and the default multiplier of three
/// puts the suggested depth past 100 ms -- above the 80 ms ceiling USB
/// permits, which is the case
/// `a_target_outside_the_new_range_is_brought_inside_it` needs to exist.
fn bursty(index: u16) -> u64 {
    if index % 2 == 0 {
        0
    } else {
        PACKET_NANOS + 60_000_000
    }
}

#[test]
fn a_retune_keeps_every_packet_it_was_holding() {
    // The whole reason this is not a new JitterBuffer. What the buffer holds
    // is audio that has arrived and not yet been played; discarding it is a
    // hole in the output at the precise moment the migration was supposed to
    // be inaudible.
    let mut buffer = buffer(USB);
    feed(&mut buffer, 0, 12, 0, smooth, false);

    // Start playing and take two, so the buffer is mid-stream rather than
    // still filling.
    assert_eq!(buffer.pop(), PopOutcome::Packet(packet(0)));
    assert_eq!(buffer.pop(), PopOutcome::Packet(packet(1)));
    let held = buffer.depth_packets();
    assert_eq!(held, 10);

    let before = buffer.stats();
    buffer.retune(WIFI);

    assert_eq!(
        buffer.depth_packets(),
        held,
        "the retune dropped audio it was holding"
    );
    assert_eq!(
        buffer.stats().shed,
        before.shed,
        "the retune shed audio of its own"
    );

    // And it is the same audio, in the same order.
    for sequence in 2..12_u16 {
        assert_eq!(
            buffer.pop(),
            PopOutcome::Packet(packet(sequence)),
            "packet {sequence} did not survive the retune"
        );
    }
    assert_eq!(buffer.stats().lost, before.lost, "a slot was given up on");
}

#[test]
fn a_retune_does_not_re_arm_a_buffer_that_is_already_playing() {
    // The dangerous direction, and the failure mode that would replace one
    // gap with another: Wi-Fi's target is five packets and the buffer coming
    // off USB holds three, so a buffer that re-armed would starve deliberately
    // until it had filled to the new depth.
    let mut buffer = buffer(USB);
    feed(&mut buffer, 0, 3, 0, smooth, false);

    // Three packets is above USB's 10 ms target, so it is playing.
    assert_eq!(buffer.pop(), PopOutcome::Packet(packet(0)));
    let starved = buffer.stats().starved;

    buffer.retune(WIFI);

    // Two packets held, against a new target of five.
    assert!(buffer.depth_ms() < buffer.target_ms());
    assert_eq!(
        buffer.pop(),
        PopOutcome::Packet(packet(1)),
        "the buffer re-armed and starved on audio it already had"
    );
    assert_eq!(buffer.stats().starved, starved);
}

#[test]
fn a_reset_is_still_the_thing_that_re_arms() {
    // The contrast that gives the test above its meaning. `reset` is for a new
    // sender or a new format, where starting over is correct; `retune` is for
    // the same stream over a different wire, where it is not.
    let mut buffer = buffer(USB);
    feed(&mut buffer, 0, 3, 0, smooth, false);
    assert_eq!(buffer.pop(), PopOutcome::Packet(packet(0)));

    buffer.reset();

    assert_eq!(buffer.depth_packets(), 0);
    assert_eq!(buffer.pop(), PopOutcome::Starved);
}

#[test]
fn usb_to_wifi_lands_on_wifi_s_floor_immediately() {
    // The migration that is worth fixing. Nothing about USB's 10 ms target is
    // illegal on Wi-Fi -- it sits between min_ms and max_ms -- so clamping the
    // held target into the new range leaves it exactly where it was, and the
    // buffer spends the next nine seconds growing into a depth it could have
    // had on the first packet.
    let mut buffer = buffer(USB);
    feed(&mut buffer, 0, 400, 0, smooth, true);
    let on_usb = buffer.target_ms();
    assert!(
        on_usb < f64::from(WIFI.target_ms),
        "a smooth USB session should sit below Wi-Fi's depth, got {on_usb} ms"
    );

    buffer.retune(WIFI);

    assert!(
        buffer.target_ms() >= f64::from(WIFI.target_ms),
        "still holding USB's depth on Wi-Fi: {} ms",
        buffer.target_ms()
    );
    assert!(
        buffer.target_ms() > on_usb,
        "the target did not move at all"
    );
    // Not one packet of adaptation was needed to get there.
    assert_eq!(buffer.stats().target_grew, 0);
}

#[test]
fn wifi_to_usb_gives_the_wasted_depth_back_immediately() {
    // The other direction. Not dangerous, but 20 ms of latency nobody asked
    // for, and the hysteresis is in no hurry to give it back: shrinking needs
    // a 1.7 ratio and then a cooldown of hundreds of packets.
    let mut buffer = buffer(WIFI);
    feed(&mut buffer, 0, 400, 0, smooth, true);
    let on_wifi = buffer.target_ms();

    buffer.retune(USB);

    assert!(
        buffer.target_ms() < on_wifi,
        "still holding Wi-Fi's {on_wifi} ms on a wire"
    );
    assert!(buffer.target_ms() <= f64::from(USB.target_ms));
}

#[test]
fn a_target_outside_the_new_range_is_brought_inside_it() {
    // A session whose link permits a deeper buffer than USB does grows its
    // target past anything USB permits, so after the migration the buffer
    // would be aiming for a depth its own `shed_over_budget` bound calls an
    // emergency.
    //
    // The ceiling here is written into the test rather than taken from a
    // shipped link, because both shipped links now stop at 80 ms and neither
    // can produce a target the other refuses. That makes this a property of
    // `retune` and not of any particular pair of constants, which is what it
    // was always meant to be: nothing stops a future link being configured
    // deeper, and a clamp that is only exercised by today's numbers is a clamp
    // nobody will notice losing.
    let deep = JitterConfig {
        max_ms: 200,
        ..WIFI
    };
    let mut buffer = buffer(deep);
    // Long enough to clear Wi-Fi's grow cooldown, which is 2500 packets: the
    // first growth happens on an estimate that has barely converged, and the
    // target only reaches what the link really costs on the growth after it.
    feed(&mut buffer, 0, 3_000, 0, bursty, true);
    let grown = buffer.target_ms();
    assert!(
        grown > f64::from(USB.max_ms),
        "the burst pattern did not grow the target past USB's ceiling: {grown} ms"
    );

    buffer.retune(USB);

    assert!(
        buffer.target_ms() >= f64::from(USB.min_ms) && buffer.target_ms() <= f64::from(USB.max_ms),
        "target {} ms is outside USB's {}..{} ms",
        buffer.target_ms(),
        USB.min_ms,
        USB.max_ms
    );
}

#[test]
fn a_retune_frees_the_target_from_the_old_link_s_cooldown() {
    // A cooldown is a promise not to react for several seconds. It was made
    // about a link that is no longer carrying the audio, and honouring it is
    // the same nine-second wait by another route.
    let mut buffer = buffer(USB);
    // A quiet stretch long enough that the buffer has retargeted at least once
    // and is inside a cooldown when the link changes.
    let (sequence, arrival) = feed(&mut buffer, 0, 400, 0, smooth, true);

    buffer.retune(WIFI);

    // One packet of the new link, jittery enough that Wi-Fi's target should
    // move. If the old cooldown still applied it could not.
    let grew_before = buffer.stats().target_grew;
    feed(&mut buffer, sequence, 60, arrival, bursty, true);
    assert!(
        buffer.stats().target_grew > grew_before,
        "the target was still locked out by the cooldown the old link earned"
    );
}

#[test]
fn the_packet_that_straddles_the_change_is_not_read_as_a_backlog() {
    // The two paths have different transit times, so the one packet either
    // side of the migration differences into a step. Read as a delivery rate
    // it looks like a socket buffer being emptied at wire speed, and the
    // buffer would shed audio down to its target on the strength of a single
    // crossing.
    let mut buffer = buffer(WIFI);
    let (sequence, arrival) = feed(&mut buffer, 0, 30, 0, smooth, false);
    let shed = buffer.stats().shed;
    assert!(buffer.depth_ms() > f64::from(USB.target_ms));

    buffer.retune(USB);

    // The first packet over the wire arrives immediately: on the old path it
    // would still have been in flight.
    buffer.push(
        sequence,
        u32::from(sequence).wrapping_mul(FRAMES_PER_PACKET),
        arrival,
        packet(sequence),
    );

    assert_eq!(
        buffer.stats().shed,
        shed,
        "the step across the migration was read as a backlog and cost audio"
    );
}

#[test]
fn a_retune_keeps_the_counters_running() {
    // The session did not restart. A shed or a loss from before the migration
    // is still something that happened to this session, and zeroing it hides
    // the link that caused it.
    let mut buffer = buffer(WIFI);
    feed(&mut buffer, 0, 40, 0, smooth, true);
    let before = buffer.stats();
    assert!(before.accepted > 0);

    buffer.retune(USB);

    let after = buffer.stats();
    assert_eq!(after.accepted, before.accepted);
    assert_eq!(after.duplicates, before.duplicates);
    assert_eq!(after.too_late, before.too_late);
    assert_eq!(after.lost, before.lost);
    assert_eq!(after.shed, before.shed);
    assert_eq!(
        JitterStats {
            target_grew: 0,
            target_shrank: 0,
            ..after
        },
        JitterStats {
            target_grew: 0,
            target_shrank: 0,
            ..before
        }
    );
}

#[test]
fn one_packet_cannot_move_the_link() {
    // Nothing authenticates this wire. A single datagram with the flag flipped
    // -- injected, or corrupted past the UDP checksum -- must not retune a
    // live buffer, because the retune costs the drift estimator its history
    // and moves the target.
    let mut watch = LinkWatch::new(Transport::WiFi);
    assert_eq!(watch.observe(Transport::Usb), None);
    assert_eq!(watch.link(), Transport::WiFi);
}

#[test]
fn a_run_of_declarations_moves_the_link_exactly_once() {
    let mut watch = LinkWatch::new(Transport::WiFi);

    for packet in 1..LINK_CONFIRMATIONS {
        assert_eq!(
            watch.observe(Transport::Usb),
            None,
            "moved on packet {packet} of {LINK_CONFIRMATIONS}"
        );
        assert_eq!(watch.link(), Transport::WiFi);
    }

    assert_eq!(watch.observe(Transport::Usb), Some(Transport::Usb));
    assert_eq!(watch.link(), Transport::Usb);

    // And then nothing, for the rest of the session on that link. The caller
    // acts on the return value, so a second Some would retune again.
    for _ in 0..100 {
        assert_eq!(watch.observe(Transport::Usb), None);
    }
}

#[test]
fn an_interrupted_run_starts_again() {
    // A stray header among honest ones is what this rejects, and a run broken
    // by an honest packet is stray by definition. A real migration is not
    // interrupted: the sender has already changed interface.
    let mut watch = LinkWatch::new(Transport::WiFi);

    for _ in 0..40 {
        for _ in 0..LINK_CONFIRMATIONS - 1 {
            assert_eq!(watch.observe(Transport::Usb), None);
        }
        assert_eq!(watch.observe(Transport::WiFi), None);
    }

    assert_eq!(
        watch.link(),
        Transport::WiFi,
        "a stray packet moved the link"
    );
}

#[test]
fn confirming_costs_less_than_a_packet_of_hesitation_is_worth() {
    // The trade the rule is making, stated as a number. Reacting this late is
    // free against the seconds a mis-sized buffer costs; reacting on one
    // packet is not free at all.
    let cost_ms = f64::from(LINK_CONFIRMATIONS) * PACKET_MS;

    // What reacting late costs instead: the adaptation cannot move the target
    // again until the cooldown it is inside expires, so a migration missed is
    // a wrongly sized buffer for that long.
    let late_ms = f64::from(WIFI.grow_cooldown_packets) * PACKET_MS;

    assert!(
        cost_ms * 100.0 < late_ms,
        "confirmation costs {cost_ms} ms against {late_ms} ms of reacting late"
    );
}
