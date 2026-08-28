//! What stops a receiver accumulating latency it can never give back.
//!
//! Audio crosses two buffers on the way to the speaker: the jitter buffer,
//! which absorbs the network, and the hand-off ring, which absorbs the gap
//! between the receive thread and the audio callback. The listener waits
//! through the sum of the two, so a bound on either half alone is not a bound
//! on anything.
//!
//! That is not a hypothetical. The ring used to absorb every surplus and the
//! ring is what `handoff::Producer::resync_if_hopeless` watches, so the total
//! was bounded by accident. Pacing the hand-off moved the surplus into the
//! jitter buffer, where the only remaining limit was `max_packets`: 256
//! packets, 1536 ms at 6 ms a packet, after which the buffer began rejecting
//! newly arrived audio to keep a second and a half of stale audio. A real
//! device sat there for minutes with the drift controller pinned at its
//! 500 ppm limit, which needed fifty of them to unwind.
//!
//! Every timeline below is synthetic: this crate has no clock, and the whole
//! session is a list of arrival times and a sink rate chosen by the test.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::handoff::{self, Consumer, Producer};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, Transport};
use sonduit_core::pacing::{drain_allowance, PacingConfig};

const RATE: u64 = 48_000;
const CHANNELS: usize = 2;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

/// The link these tests run on, and the source of every number asserted
/// against below. Nothing here is a constant of its own.
const LINK: JitterConfig = JitterConfig::for_transport(Transport::Usb);

/// What the receive thread in `sonduit-ffi` is configured with. Duplicated
/// rather than shared because that crate cannot be built for this test host,
/// and asserting against numbers nobody ships would prove nothing.
const PACING: PacingConfig = PacingConfig {
    floor_packets: 2,
    max_per_packet: 3,
};

/// The most the ring can hold under that pacing.
///
/// The hand-off is refilled only below the floor and by at most
/// `max_per_packet` packets at a time, so this is the highest level any
/// arrival can leave it at.
fn ring_ceiling_ms() -> f64 {
    (PACING.floor_packets + PACING.max_per_packet) as f64 * PACKET_MS
}

/// The most the receiver may hold across both buffers.
///
/// `max_ms` is the budget. The extra packet is granularity and nothing else:
/// the shed lands on the target, which is a whole number of packets, and one
/// arrival can be added between two checks.
fn budget_ms() -> f64 {
    f64::from(LINK.max_ms) + PACKET_MS
}

/// What the buffer would have run to with nothing bounding it, which is the
/// number the real device reported.
fn hard_ceiling_ms() -> f64 {
    LINK.max_packets as f64 * PACKET_MS
}

fn packet() -> Vec<u8> {
    vec![1_u8; PCM_PAYLOAD_BYTES]
}

/// One session, played out against a timeline the test controls.
struct Session {
    buffer: JitterBuffer,
    queue: Producer,
    device: Consumer,
    /// Simulated time the device has already been fed up to.
    played_to: u64,
    /// How fast the device consumes, as a fraction of real time.
    ///
    /// One is a device whose clock agrees with the sender's. Below one is a
    /// device that plays slower than the audio is produced, which is drift the
    /// resampler corrects up to 500 ppm and cannot correct beyond. Zero is a
    /// callback that has stopped running.
    speed: f64,
    /// Packets taken by each shed that discarded something.
    sheds: Vec<u64>,
    /// The most the two buffers held together at any point in the session.
    peak_held_ms: f64,
}

impl Session {
    fn new() -> Self {
        let format = Format::stereo_48k();
        let (queue, device) = handoff::channel(format, 400);
        Self {
            buffer: JitterBuffer::new(format, LINK),
            queue,
            device,
            played_to: 0,
            speed: 1.0,
            sheds: Vec::new(),
            peak_held_ms: 0.0,
        }
    }

    /// Let the audio callback take everything its own clock entitles it to, up
    /// to `now`.
    ///
    /// It is the only thing here that runs on the wall clock rather than on
    /// packet arrivals, which is the asymmetry every failure below lives in.
    fn play_until(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.played_to);
        self.played_to = now;
        let consumed = (elapsed as f64 * self.speed) as u64;
        let frames = (consumed * RATE / 1_000_000_000) as usize;
        if frames == 0 {
            return;
        }
        let mut out = vec![0_i16; frames * CHANNELS];
        self.device.fill(&mut out, frames);
    }

    /// One arrival, handled exactly as `receive_loop` handles one.
    fn receive(&mut self, sequence: u32, arrival: u64) {
        self.play_until(arrival);

        self.buffer.push(
            sequence as u16,
            sequence.wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            packet(),
        );

        // The bound, on the receive thread, with both depths in hand.
        let shed = self.buffer.shed_over_budget(self.queue.queued_ms());
        if shed > 0 {
            self.sheds.push(shed);
        }

        let allowance = drain_allowance(self.queue.queued_ms(), PACKET_MS, PACING);
        for _ in 0..allowance {
            match self.buffer.pop() {
                PopOutcome::Packet(pcm) => {
                    if self.queue.push(&pcm) < pcm.len() {
                        break;
                    }
                }
                // Concealment is silence of one packet's duration here. What
                // it contains does not matter to a latency test; that it takes
                // up the same room as the packet it stands in for does.
                PopOutcome::Lost => {
                    let silence = vec![0_u8; PCM_PAYLOAD_BYTES];
                    if self.queue.push(&silence) < silence.len() {
                        break;
                    }
                }
                PopOutcome::Starved => break,
            }
        }

        let held = self.held_ms();
        if held > self.peak_held_ms {
            self.peak_held_ms = held;
        }
    }

    /// Audio the receiver is holding, across both buffers.
    ///
    /// This is what the listener waits through, and holding it in one rather
    /// than the other changes nothing about that.
    fn held_ms(&self) -> f64 {
        self.buffer.depth_ms() + self.queue.queued_ms()
    }
}

/// A session where the sender produces a packet every 6 ms and never varies,
/// and the device runs at `speed` for the whole of it.
fn run(speed: f64, packets: u32) -> Session {
    let mut session = Session::new();
    session.speed = speed;
    for sequence in 0..packets {
        session.receive(sequence, u64::from(sequence) * PACKET_NANOS);
    }
    session
}

#[test]
fn a_source_that_outruns_the_sink_stops_at_the_bound() {
    // A device playing ten per cent slow. That is two hundred times what the
    // resampler can correct, so the one continuous actuator in the system is
    // pinned and losing, which is the state the real session was logged in.
    let packets = 3_000;
    let session = run(0.9, packets);

    // Where the depth would have gone: every millisecond the sink failed to
    // consume stayed in the buffers, and 3000 packets of it is well past the
    // point where the buffer starts refusing new audio to keep old.
    let surplus_ms = f64::from(packets) * PACKET_MS * 0.1;
    assert!(
        surplus_ms > hard_ceiling_ms(),
        "the simulation is too short to reach the failure it is about: \
         {surplus_ms:.0} ms of surplus against a {:.0} ms ceiling",
        hard_ceiling_ms()
    );

    assert!(
        session.peak_held_ms <= budget_ms() + ring_ceiling_ms(),
        "held {:.0} ms at the peak, against a {:.0} ms budget and a ring that \
         cannot exceed {:.0} ms",
        session.peak_held_ms,
        budget_ms(),
        ring_ceiling_ms()
    );

    // And it is still there at the end rather than having drifted up to it
    // once and stayed: the bound is where the session lives, not where it
    // stopped growing.
    assert!(
        session.held_ms() <= budget_ms() + ring_ceiling_ms(),
        "held {:.0} ms at the end",
        session.held_ms()
    );

    assert!(
        !session.sheds.is_empty(),
        "nothing was shed against a sink the resampler cannot catch"
    );

    // Each shed buys back the whole difference between the budget and the
    // target, so they are far apart. Shedding on a substantial fraction of
    // arrivals would be dropping audio routinely, which is the thing this must
    // never become.
    assert!(
        session.sheds.len() < packets as usize / 50,
        "shed {} times in {packets} packets",
        session.sheds.len()
    );
}

#[test]
fn a_healthy_session_is_never_shed() {
    // The same simulation with a device whose clock agrees with the sender's,
    // which is every session that is behaving.
    let session = run(1.0, 3_000);
    let stats = session.buffer.stats();

    assert_eq!(
        stats.shed, 0,
        "shed {} packets on a healthy link",
        stats.shed
    );
    assert_eq!(stats.too_late, 0, "rejected audio on a healthy link");
    assert!(
        session.peak_held_ms < f64::from(LINK.max_ms),
        "a healthy session peaked at {:.0} ms, inside the budget only by luck",
        session.peak_held_ms
    );
}

#[test]
fn the_depths_a_behaving_device_reported_are_left_alone() {
    // The measured numbers from before the regression, asserted directly
    // rather than through a simulation: 42 ms of jitter buffer against 12 ms
    // of ring. Nothing may fire here.
    let mut buffer = JitterBuffer::new(Format::stereo_48k(), LINK);
    for sequence in 0..7_u32 {
        buffer.push(
            sequence as u16,
            sequence.wrapping_mul(FRAMES_PER_PACKET),
            u64::from(sequence) * PACKET_NANOS,
            packet(),
        );
    }

    assert!(
        (buffer.depth_ms() - 42.0).abs() < 0.01,
        "{}",
        buffer.depth_ms()
    );
    let ring_ms = PACING.floor_packets as f64 * PACKET_MS;
    assert_eq!(buffer.shed_over_budget(ring_ms), 0);
    assert_eq!(buffer.stats().shed, 0);
}

#[test]
fn the_bound_comes_from_the_configuration() {
    // Two buffers holding exactly the same audio, differing only in what their
    // link says it may hold. If the bound were a constant of its own they
    // would behave the same.
    let mut generous = JitterBuffer::new(
        Format::stereo_48k(),
        JitterConfig::for_transport(Transport::WiFi),
    );
    let mut strict = JitterBuffer::new(
        Format::stereo_48k(),
        JitterConfig {
            max_ms: 40,
            ..JitterConfig::for_transport(Transport::WiFi)
        },
    );

    for sequence in 0..20_u32 {
        let timestamp = sequence.wrapping_mul(FRAMES_PER_PACKET);
        let arrival = u64::from(sequence) * PACKET_NANOS;
        generous.push(sequence as u16, timestamp, arrival, packet());
        strict.push(sequence as u16, timestamp, arrival, packet());
    }

    // 120 ms held: inside 200 ms and well outside 40 ms.
    assert!((generous.depth_ms() - 120.0).abs() < 0.01);
    assert_eq!(generous.shed_over_budget(12.0), 0);
    assert!(strict.shed_over_budget(12.0) > 0);
}

#[test]
fn a_stalled_device_is_shed_once_and_then_left_alone() {
    // The callback stops for a hundred milliseconds and comes back. Audio goes
    // on arriving throughout, because the sender has no idea. This is the
    // shape of every real cause: a device that opens slowly, a phone that
    // schedules the audio thread late, a stream that has to be reopened.
    let mut session = Session::new();
    let stall_from = 200_u32;
    let stall_packets = 100 / PACKET_MS as u32;

    for sequence in 0..600_u32 {
        session.speed = if (stall_from..stall_from + stall_packets).contains(&sequence) {
            0.0
        } else {
            1.0
        };
        session.receive(sequence, u64::from(sequence) * PACKET_NANOS);
    }

    assert_eq!(
        session.sheds.len(),
        1,
        "shed {} times for one stall: {:?}",
        session.sheds.len(),
        session.sheds
    );

    // Nothing after the stall. The shed brought the total back under the
    // budget in one step and the session ran the remaining three seconds
    // without another, which is the difference between an emergency and a
    // policy.
    let held = session.held_ms();
    assert!(
        held <= budget_ms(),
        "left holding {held:.0} ms after the stall"
    );
    assert!(
        held >= PACING.floor_packets as f64 * PACKET_MS,
        "left holding only {held:.0} ms, which is below the queue's own floor"
    );

    // What is left is not zero, and it is not meant to be: shedding lands on
    // the target and stops. Anything above that is latency the resampler is
    // there to unwind, slowly and without a click.
    assert!(session.buffer.stats().starved < 600, "playback stalled out");
}
