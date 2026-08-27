//! Does drift correction actually keep the buffer at target?
//!
//! The unit tests check the controller and the resampler separately. This
//! checks the thing that matters: a sender whose crystal differs from the
//! receiver's, running for long enough that the difference is fatal, and a
//! buffer that survives it.
//!
//! The numbers here are the ones from `docs/roadmap.md`: at 50 ppm with 30 ms
//! of headroom an uncorrected buffer leaves its usable range in ten minutes.
//! Which way it leaves depends on the sign. A sender running slow drains the
//! buffer until it underruns; a sender running fast fills it, doubling the
//! latency the user hears and eventually overflowing. Neither is survivable,
//! and the correction has to handle both.

use sonduit_core::format::Format;
use sonduit_core::ratio::{RatioConfig, RatioController};
use sonduit_core::resample::DriftResampler;

/// Frames in one packet at 48 kHz stereo 16-bit, from the 1152-byte payload.
const CHUNK: usize = 288;

/// Milliseconds one packet carries.
const PACKET_MS: f64 = CHUNK as f64 * 1000.0 / 48_000.0;

/// Packets between corrections, matching what the receive loop uses.
const PER_CORRECTION: usize = 40;

const TARGET_MS: f64 = 30.0;

/// Simulate a session and return the buffer depth over time, in milliseconds.
///
/// `drift_ppm` is positive when the sender runs fast. `correct` selects whether
/// the controller is allowed to act, so the two runs differ in exactly one
/// thing.
fn simulate(drift_ppm: f64, packets: usize, correct: bool) -> Vec<f64> {
    let mut controller = RatioController::new(RatioConfig::default());
    let mut depth_ms = TARGET_MS;
    let mut history = Vec::with_capacity(packets / PER_CORRECTION + 1);

    for packet in 0..packets {
        let ratio = if correct { controller.ratio() } else { 1.0 };

        // The sender emits one packet of audio on its own clock. A sender
        // running fast delivers it in less receiver-time than it contains, so
        // the receiver has consumed less than a packet by the time it lands.
        let delivered_ms = PACKET_MS * ratio;
        let consumed_ms = PACKET_MS / (1.0 + drift_ppm * 1e-6);
        depth_ms += delivered_ms - consumed_ms;

        if packet % PER_CORRECTION == 0 {
            controller.update(depth_ms, TARGET_MS, Some(drift_ppm));
            history.push(depth_ms);
        }
    }

    history
}

#[test]
fn an_uncorrected_buffer_leaves_its_range_within_ten_minutes() {
    // The premise the whole feature rests on. If this ever stops being true
    // the correction is solving a problem that no longer exists, and this test
    // is where that would show up.
    //
    // Ten minutes at 6 ms a packet. Fifty ppm displaces the buffer by
    // 600 s * 50e-6 = 30 ms, which is the entire headroom.
    let packets = (600.0 / (PACKET_MS / 1000.0)) as usize;

    // The whole 30 ms is gone, to within the rounding of a ten-minute sum.
    let slow = *simulate(-50.0, packets, false).last().unwrap();
    assert!(
        slow < 1.0,
        "a slow sender left {slow:.1} ms in the buffer, so it never underran"
    );

    let fast = *simulate(50.0, packets, false).last().unwrap();
    assert!(
        fast >= TARGET_MS * 2.0 - 1.0,
        "a fast sender left {fast:.1} ms in the buffer, so the latency never doubled"
    );
}

#[test]
fn correction_holds_the_buffer_at_target_over_the_same_run() {
    let packets = (600.0 / (PACKET_MS / 1000.0)) as usize;
    let history = simulate(50.0, packets, true);

    let final_depth = *history.last().unwrap();
    assert!(
        (final_depth - TARGET_MS).abs() < 3.0,
        "corrected buffer settled at {final_depth:.1} ms, not near {TARGET_MS} ms"
    );

    // Settling is not enough on its own: a buffer that dips to zero on the way
    // has already been heard to underrun.
    let lowest = history.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(
        lowest > 10.0,
        "the buffer dipped to {lowest:.1} ms during settling"
    );
}

#[test]
fn correction_works_in_both_directions() {
    // A sender running slow drains nothing; it floods, and a buffer that grows
    // without bound is latency that never comes back.
    let packets = (600.0 / (PACKET_MS / 1000.0)) as usize;

    for drift in [-50.0, -20.0, 20.0, 50.0] {
        let history = simulate(drift, packets, true);
        let final_depth = *history.last().unwrap();
        assert!(
            (final_depth - TARGET_MS).abs() < 3.0,
            "at {drift} ppm the buffer settled at {final_depth:.1} ms"
        );
    }
}

#[test]
fn a_drift_beyond_anything_physical_is_survived_rather_than_corrected() {
    // 5000 ppm is not a crystal, it is a bug somewhere upstream. The
    // correction is clamped, so the buffer still drains; what must not happen
    // is the controller producing a ratio that distorts the audio while
    // failing anyway.
    let packets = (60.0 / (PACKET_MS / 1000.0)) as usize;
    let mut controller = RatioController::new(RatioConfig::default());

    for _ in 0..packets {
        controller.update(0.0, TARGET_MS, Some(5_000.0));
    }

    assert!(
        controller.correction_ppm().abs() <= 500.0,
        "correction ran to {} ppm",
        controller.correction_ppm()
    );
    assert!(controller.saturated(), "the fault was not detectable");
}

#[test]
fn the_resampler_produces_the_frame_count_the_controller_asked_for() {
    // The controller can be right and the correction still not happen, if the
    // resampler does not deliver the ratio it was given. Over a minute of
    // audio at 50 ppm the difference is 144 frames, so this is measured over a
    // long enough run to be unambiguous.
    let format = Format::stereo_48k();
    let mut resampler = DriftResampler::new(format, CHUNK).unwrap();
    resampler.set_ratio(1.000_05);

    let mut input_frames = 0_usize;
    let mut output_frames = 0_usize;

    // A tone rather than silence: silence resamples to silence whatever the
    // ratio is, so it would prove nothing.
    let mut phase = 0_usize;
    for _ in 0..10_000 {
        let mut chunk = Vec::with_capacity(CHUNK * 4);
        for _ in 0..CHUNK {
            let value = (2.0 * std::f64::consts::PI * 440.0 * phase as f64 / 48_000.0).sin();
            let sample = (value * 0.5 * f64::from(i16::MAX)) as i16;
            chunk.extend_from_slice(&sample.to_le_bytes());
            chunk.extend_from_slice(&sample.to_le_bytes());
            phase += 1;
        }

        let out = resampler.process(&chunk).unwrap();
        input_frames += CHUNK;
        output_frames += out.len() / 4;
    }

    let measured = output_frames as f64 / input_frames as f64;
    assert!(
        (measured - 1.000_05).abs() < 5e-6,
        "asked for 1.000050, measured {measured:.7}"
    );
}
