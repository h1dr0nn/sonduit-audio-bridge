//! What a device that opens slowly does to the latency for the rest of the
//! session.
//!
//! On a real USB session the phone held 86 to 116 ms in the hand-off ring,
//! permanently. The receive thread had called AAudio's open synchronously, the
//! kernel's socket buffer had gone on accepting datagrams for the tens of
//! milliseconds that took, and the loop then drained that backlog at wire
//! speed: one packet handed across per datagram read, while the audio device
//! had consumed almost nothing in real time.
//!
//! Nothing about that is specific to AAudio or to a socket, which is why it
//! can be reproduced here. The timeline below is synthetic and this crate has
//! no clock: the whole session is a list of arrival times chosen by the test.
//!
//! Every run is the same simulation with one number changed -- how long the
//! device takes to open -- so the difference between them is the defect and
//! nothing else.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::handoff::{self, Consumer, Producer};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, Transport};
use sonduit_core::pacing::{drain_allowance, PacingConfig};

const RATE: u64 = 48_000;
const CHANNELS: usize = 2;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

/// What the receive thread in `sonduit-ffi` is configured with. Duplicated
/// rather than shared because that crate cannot be built for this test host,
/// and asserting against numbers nobody ships would prove nothing.
const PACING: PacingConfig = PacingConfig {
    floor_packets: 2,
    max_per_packet: 3,
};

/// How long one datagram takes to read and decode once it is already waiting.
///
/// This is the whole mechanism: it is three hundred times smaller than the
/// 6 ms of audio the datagram carries, so a queue of them empties into the
/// receiver far faster than any device could play it.
const WIRE_SPEED_NANOS: u64 = 20_000;

/// One second of session at 6 ms a packet, which is long enough for both
/// buffers to settle wherever the startup left them.
const PACKETS: u16 = 167;

/// Every sample of packet `n` carries `n`, so the audio coming out of the
/// device can be read back as the list of packets that reached it.
fn packet(sequence: u16) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(PCM_PAYLOAD_BYTES);
    for _ in 0..PCM_PAYLOAD_BYTES / 2 {
        pcm.extend_from_slice(&(sequence as i16).to_le_bytes());
    }
    pcm
}

/// One session, played out against a timeline the test controls.
struct Session {
    buffer: JitterBuffer,
    queue: Option<Producer>,
    device: Option<Consumer>,
    /// Simulated time the device has already been fed up to.
    played_to: u64,
    /// Every sample the device was handed, in order.
    heard: Vec<i16>,
}

impl Session {
    fn new() -> Self {
        Self {
            buffer: JitterBuffer::new(
                Format::stereo_48k(),
                JitterConfig::for_transport(Transport::Usb),
            ),
            queue: None,
            device: None,
            played_to: 0,
            heard: Vec::new(),
        }
    }

    /// Open the audio device: the hand-off is created and the callback starts
    /// pulling from `now` onwards. This is `open_playback`.
    fn open(&mut self, now: u64) {
        let (producer, consumer) = handoff::channel(Format::stereo_48k(), 400);
        self.queue = Some(producer);
        self.device = Some(consumer);
        self.played_to = now;
    }

    /// Let the audio callback take everything real time entitles it to, up to
    /// `now`.
    ///
    /// It is the only thing in the simulation that runs on the wall clock
    /// rather than on packet arrivals, which is the asymmetry the whole defect
    /// lives in.
    fn play_until(&mut self, now: u64) {
        let Some(device) = self.device.as_mut() else {
            return;
        };
        let elapsed = now.saturating_sub(self.played_to);
        self.played_to = now;
        let frames = (elapsed * RATE / 1_000_000_000) as usize;
        if frames == 0 {
            return;
        }
        let mut out = vec![0_i16; frames * CHANNELS];
        let written = device.fill(&mut out, frames);
        self.heard.extend_from_slice(&out[..written * CHANNELS]);
    }

    /// One arrival, handled exactly as `receive_loop` handles one.
    fn receive(&mut self, sequence: u16, arrival: u64) {
        self.play_until(arrival);

        self.buffer.push(
            sequence,
            u32::from(sequence).wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            packet(sequence),
        );

        let Some(queue) = self.queue.as_mut() else {
            return;
        };
        let allowance = drain_allowance(queue.queued_ms(), PACKET_MS, PACING);
        for _ in 0..allowance {
            match self.buffer.pop() {
                PopOutcome::Packet(pcm) => {
                    if queue.push(&pcm) < pcm.len() {
                        break;
                    }
                }
                // Concealment is silence of one packet's duration here. What
                // it contains does not matter to a latency test; that it takes
                // up the same room as the packet it stands in for does.
                PopOutcome::Lost => {
                    let silence = vec![0_u8; PCM_PAYLOAD_BYTES];
                    if queue.push(&silence) < silence.len() {
                        break;
                    }
                }
                PopOutcome::Starved => break,
            }
        }
    }

    /// End the session: hand over everything still buffered and let the device
    /// play it out.
    ///
    /// The drain is arrival-driven, so without this the last few packets are
    /// still in the jitter buffer when the sender stops, and `heard` would be
    /// missing its own tail. Nothing about latency is asserted after this.
    fn finish(&mut self) {
        if let Some(queue) = self.queue.as_mut() {
            while let PopOutcome::Packet(pcm) = self.buffer.pop() {
                if queue.push(&pcm) < pcm.len() {
                    break;
                }
            }
        }
        self.play_until(self.played_to + 10 * 1_000_000_000);
    }

    /// Audio the receiver is holding, across both buffers.
    ///
    /// This is what the listener waits through, and holding it in one rather
    /// than the other changes nothing about that.
    fn held_ms(&self) -> f64 {
        self.buffer.depth_ms() + self.queue.as_ref().map_or(0.0, Producer::queued_ms)
    }

    /// The packets the device actually played, in order, with the runs of
    /// repeated samples collapsed.
    fn played(&self) -> Vec<i16> {
        let mut out: Vec<i16> = Vec::new();
        for sample in &self.heard {
            if out.last() != Some(sample) {
                out.push(*sample);
            }
        }
        out
    }
}

/// Run a session whose device takes `open_ms` to open, for `packets` packets.
///
/// The sender produces a packet every 6 ms from time zero and never varies.
/// Everything that differs between one run and another follows from `open_ms`.
fn run(open_ms: u64, packets: u16) -> Session {
    let mut session = Session::new();
    let open_nanos = open_ms * 1_000_000;

    // The first packet is what tells the receiver the format, so the device is
    // opened from inside the handling of it. The thread is inside that call
    // until `open_nanos`; datagrams keep arriving at the socket meanwhile and
    // are read back to back the moment it returns.
    session.receive(0, 0);
    session.open(open_nanos);
    let mut backlog = 0_u64;

    for sequence in 1..packets {
        let produced = u64::from(sequence) * PACKET_NANOS;
        let arrival = if produced < open_nanos {
            // Waiting in the socket buffer. Read at wire speed, in the order it
            // was queued, as soon as the thread is free.
            backlog += 1;
            open_nanos + backlog * WIRE_SPEED_NANOS
        } else {
            // Real time again, or an open short enough that the thread never
            // fell behind at all.
            produced.max(open_nanos + backlog * WIRE_SPEED_NANOS)
        };
        session.receive(sequence, arrival);
    }

    // Deliberately no trailing drain. What the session is holding when the last
    // packet lands is the number this whole exercise is about, and playing it
    // out would report zero.
    session
}

/// The depth this session is designed to hold, in milliseconds.
///
/// The jitter buffer's own target, rounded up to the whole packets it deals in,
/// plus the audio queue's floor. The queue gets two packets of slack rather
/// than none: the pacing rule hands one more over whenever the queue is within
/// a packet of its floor, so the depth immediately after a hand-over sits a
/// packet above the band it is being held in.
fn budget_ms(session: &Session) -> f64 {
    let buffer = (session.buffer.target_ms() / PACKET_MS).ceil() * PACKET_MS;
    let queue = (PACING.floor_packets + 2) as f64 * PACKET_MS;
    buffer + queue
}

#[test]
fn a_device_that_opens_late_does_not_leave_its_backlog_in_the_buffers() {
    // 110 ms of open. At 6 ms a packet that is eighteen datagrams waiting in
    // the socket before the receive thread reads any of them, which is the
    // session that was measured.
    let session = run(110, PACKETS);
    let stats = session.buffer.stats();
    let held = session.held_ms();
    let budget = budget_ms(&session);

    assert!(
        stats.shed > 0,
        "the backlog was not recognised: nothing was shed"
    );

    // The number the change is about. A second into the session the receiver
    // is holding what it is designed to hold, and not that plus the burst.
    assert!(
        held <= budget,
        "held {held:.1} ms against a {budget:.1} ms budget"
    );

    // Stated the other way round as well, because the assertion above would
    // also pass if the audio had simply stopped.
    assert!(
        held >= PACING.floor_packets as f64 * PACKET_MS,
        "held only {held:.1} ms, which is less than the queue's own floor"
    );

    // What the same session held before this change. Nothing was discarded
    // then, so everything shed here was held there instead -- and one in, one
    // out meant it went on being held for as long as the session ran. This is
    // the 86 to 116 ms that was measured on the device.
    let previously = held + stats.shed as f64 * PACKET_MS;
    assert!(
        previously > 100.0,
        "the simulation did not reproduce the defect: it would have held {previously:.1} ms"
    );
}

#[test]
fn what_is_discarded_is_discarded_once_and_early() {
    let mut session = run(110, PACKETS);
    session.finish();
    let played = session.played();

    // Every packet the device played is later than the one before it. The
    // discard takes from the front of the buffer, so the audio that survives
    // is still in order and still whole.
    for pair in played.windows(2) {
        assert!(
            pair[1] > pair[0],
            "packets came out of order: {} then {}",
            pair[0],
            pair[1]
        );
    }

    // One join, not a stream of them. Everything after the startup burst is
    // contiguous, which is the difference between shedding a backlog once and
    // dropping audio continuously to keep up.
    let joins = played
        .windows(2)
        .filter(|pair| pair[1] != pair[0] + 1)
        .count();
    assert_eq!(joins, 1, "audio was discarded {joins} times, not once");

    let (from, to) = played
        .windows(2)
        .find(|pair| pair[1] != pair[0] + 1)
        .map(|pair| (pair[0], pair[1]))
        .expect("a join");

    // And it is at the start, in the first fifty milliseconds of the session,
    // where the alternative is carrying its cost for all the rest of it.
    assert!(
        from < 8,
        "the join is at packet {from}, {:.0} ms into the session",
        f64::from(from) * PACKET_MS
    );

    // Bounded by the burst: nothing is discarded that did not arrive while the
    // device was opening.
    let skipped = f64::from(to - from - 1) * PACKET_MS;
    assert!(
        skipped < 120.0,
        "discarded {skipped:.0} ms, which is more than the device spent opening"
    );

    // The session played out to its last packet. Whatever was shed was shed
    // once, and the audio after it is all there.
    assert_eq!(played.last(), Some(&((PACKETS - 1) as i16)));
}

#[test]
fn a_session_that_starts_cleanly_is_untouched() {
    // The same simulation with a device that opens instantly, which is a
    // session with no backlog to recognise. Every packet arrives one packet's
    // duration after the last, so nothing is ever ahead of real time and the
    // rule cannot fire.
    let mut session = run(0, PACKETS);
    let held = session.held_ms();
    let budget = budget_ms(&session);
    let stats = session.buffer.stats();

    assert_eq!(
        stats.shed, 0,
        "shed {} packets on a clean start",
        stats.shed
    );
    assert_eq!(
        stats.lost, 0,
        "lost {} packets on a clean start",
        stats.lost
    );
    assert_eq!(
        stats.too_late, 0,
        "rejected {} packets on a clean start",
        stats.too_late
    );
    assert!(
        held <= budget,
        "held {held:.1} ms against a {budget:.1} ms budget"
    );

    session.finish();

    // Byte for byte: what the device played is every packet the sender
    // produced, in order, with nothing missing and nothing repeated.
    //
    // Every packet but the last one, which is still in the buffer: playback
    // re-arms at the target depth, the sender has stopped, and one packet is
    // not two. That is the end of the stream behaving as it always has, and it
    // is here in the expectation rather than papered over by draining past it.
    let played = session.played();
    let expected: Vec<i16> = (0..PACKETS as i16 - 1).collect();
    assert_eq!(played, expected, "the audio the device played changed");

    // Every sample of it, not merely every packet: one packet's worth of
    // frames for each, and no partial packet spliced in.
    assert_eq!(
        session.heard.len(),
        usize::from(PACKETS - 1) * FRAMES_PER_PACKET as usize * CHANNELS,
        "the device was handed a different number of samples"
    );
}

#[test]
fn every_open_length_ends_inside_the_budget() {
    // The open time is the one thing that differs between a session that starts
    // cleanly and the one that was measured, so it is the thing to sweep. A
    // rule that only held at the length it was written against would be a rule
    // fitted to a single trace.
    for open_ms in [0, 1, 5, 6, 12, 25, 50, 110, 200, 400] {
        let session = run(open_ms, PACKETS);
        let held = session.held_ms();
        let budget = budget_ms(&session);
        assert!(
            held <= budget,
            "a {open_ms} ms open left {held:.1} ms held against a {budget:.1} ms budget"
        );

        // And nothing is shed for an open too short to have caused a backlog:
        // under one packet, no datagram can have been waiting when the thread
        // came back.
        if open_ms < 6 {
            assert_eq!(
                session.buffer.stats().shed,
                0,
                "a {open_ms} ms open shed audio it had no backlog to shed"
            );
        }
    }
}
