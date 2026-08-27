//! Does the buffer stop chasing the jitter estimate?
//!
//! A target that grows and shrinks on the same rule oscillates: it shrinks the
//! moment the link looks calm, underruns on the next burst, grows again, and
//! the user hears every cycle. These tests drive the buffer with arrival
//! patterns that produce exactly that, and assert it does not happen.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, Transport};

const RATE: u64 = 48_000;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;

/// Nanoseconds one packet of audio occupies.
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;

fn buffer(config: JitterConfig) -> JitterBuffer {
    JitterBuffer::new(Format::stereo_48k(), config)
}

/// Push `count` packets, each arriving `spacing(i)` nanoseconds after the last.
///
/// Returns the arrival time reached, so a caller can continue a session with a
/// different pattern.
fn feed(
    buffer: &mut JitterBuffer,
    from_sequence: u16,
    count: u16,
    mut arrival: u64,
    spacing: impl Fn(u16) -> u64,
) -> (u16, u64) {
    let mut sequence = from_sequence;
    for index in 0..count {
        arrival += spacing(index);
        buffer.push(
            sequence,
            u32::from(sequence).wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            vec![0_u8; PCM_PAYLOAD_BYTES],
        );
        sequence = sequence.wrapping_add(1);
        // Drain as a player would, so the buffer does not simply fill up.
        let _ = buffer.pop();
    }
    (sequence, arrival)
}

/// Perfectly even arrivals.
fn smooth(_: u16) -> u64 {
    PACKET_NANOS
}

/// Arrivals that bunch up and then stall, which is what a contended access
/// point does to a station: several packets land together, then nothing for
/// 40 ms while the medium is busy.
///
/// Deliberately extreme. The RFC 3550 estimator converges on the mean absolute
/// change in transit time, and with the default multiplier of three the target
/// only exceeds the 30 ms floor once that mean passes 8 ms. Anything gentler
/// would leave these tests asserting on a target that never moved.
fn bursty(index: u16) -> u64 {
    if index % 2 == 0 {
        0
    } else {
        PACKET_NANOS + 40_000_000
    }
}

#[test]
fn a_burst_of_jitter_grows_the_target() {
    // The premise. If jitter never moved the target there would be nothing to
    // stabilise and these tests would be measuring nothing.
    let mut buffer = buffer(JitterConfig::for_transport(Transport::WiFi));
    let start = buffer.target_ms();

    feed(&mut buffer, 0, 400, 0, bursty);

    assert!(
        buffer.target_ms() > start,
        "target stayed at {start} ms through heavy jitter"
    );
}

#[test]
fn a_calm_link_does_not_immediately_give_the_depth_back() {
    // The oscillation this exists to prevent: shrink as soon as it looks calm,
    // underrun on the next burst, grow again.
    let mut buffer = buffer(JitterConfig::for_transport(Transport::WiFi));

    let (sequence, arrival) = feed(&mut buffer, 0, 400, 0, bursty);
    let grown = buffer.target_ms();
    assert!(
        grown > 30.0,
        "the link never looked bad enough, got {grown}"
    );

    // Two hundred smooth packets is over a second of calm.
    feed(&mut buffer, sequence, 200, arrival, smooth);

    assert_eq!(
        buffer.target_ms(),
        grown,
        "the target gave back {grown} ms of headroom after a second of calm"
    );
}

#[test]
fn a_link_that_really_improves_is_eventually_followed() {
    // Hysteresis that never releases is not hysteresis, it is a ratchet, and
    // a session that hit one bad patch would carry the latency forever.
    let mut config = JitterConfig::for_transport(Transport::WiFi);
    // Shortened so the test does not have to push thousands of packets; the
    // rule under test is the threshold and the wait, not their exact size.
    config.grow_cooldown_packets = 50;
    config.shrink_cooldown_packets = 20;
    let mut buffer = buffer(config);

    let (sequence, arrival) = feed(&mut buffer, 0, 300, 0, bursty);
    let grown = buffer.target_ms();

    feed(&mut buffer, sequence, 600, arrival, smooth);

    assert!(
        buffer.target_ms() < grown,
        "the target never came back down from {grown} ms"
    );
}

#[test]
fn alternating_conditions_do_not_make_the_target_oscillate() {
    // The measurable form of the complaint. Conditions flip every 120 packets,
    // which is under a second; with symmetric adjustment and no cooldown the
    // target would follow every flip, and the user would hear each one. The
    // shipped cooldowns are what stop it, and they are the ones used here.
    let mut buffer = buffer(JitterConfig::for_transport(Transport::WiFi));

    let mut sequence = 0_u16;
    let mut arrival = 0_u64;
    for round in 0..10 {
        let pattern: fn(u16) -> u64 = if round % 2 == 0 { bursty } else { smooth };
        let (next_sequence, next_arrival) = feed(&mut buffer, sequence, 120, arrival, pattern);
        sequence = next_sequence;
        arrival = next_arrival;
    }

    let stats = buffer.stats();
    let moves = stats.target_grew + stats.target_shrank;
    assert!(
        moves <= 4,
        "the target moved {moves} times across ten changes of conditions",
    );
}

#[test]
fn usb_settles_lower_than_wifi_on_the_same_arrivals() {
    // The transport choice has to survive the adaptation: a USB session that
    // adapts its way up to the Wi-Fi depth has thrown away the reason for
    // distinguishing them.
    let mut wifi = buffer(JitterConfig::for_transport(Transport::WiFi));
    let mut usb = buffer(JitterConfig::for_transport(Transport::Usb));

    feed(&mut wifi, 0, 400, 0, smooth);
    feed(&mut usb, 0, 400, 0, smooth);

    assert!(
        usb.target_ms() < wifi.target_ms(),
        "usb settled at {} ms, wifi at {} ms",
        usb.target_ms(),
        wifi.target_ms()
    );
}

#[test]
fn a_reset_returns_the_target_to_its_starting_depth() {
    // A reset happens on a format change or a new stream. Carrying a target
    // learned from a different link would be carrying a measurement of
    // something that no longer exists.
    let mut buffer = buffer(JitterConfig::for_transport(Transport::WiFi));
    feed(&mut buffer, 0, 400, 0, bursty);
    assert!(buffer.target_ms() > 30.0);

    buffer.reset();

    assert_eq!(buffer.target_ms(), 30.0);
}
