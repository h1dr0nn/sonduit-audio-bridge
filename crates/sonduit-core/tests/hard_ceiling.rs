//! What the buffer does when it hits the one limit it enforces on itself.
//!
//! `JitterConfig::max_ms` is the depth policy, and `shed_over_budget` is how a
//! caller applies it. `JitterConfig::max_packets` is underneath both: a bound
//! on memory that the buffer honours whether or not anyone asked it to. On a
//! receiver that applies the budget on every arrival the ceiling is
//! unreachable, and `latency_bound.rs` asserts exactly that.
//!
//! It is not unreachable in general, which is why the behaviour at it matters.
//! The budget is applied by the caller, and a caller can fail to apply it:
//! `sonduit-ffi` checks it inside the block that needs a playback queue, and
//! off Android there is never a queue, so the whole bound is skipped for the
//! life of the session. Any other embedder of this crate is free to never call
//! it at all, and a `max_ms` of zero turns it off from the configuration side.
//! In every one of those cases the packet ceiling is the only thing left.
//!
//! At the ceiling the buffer used to refuse the packet that had just arrived.
//! That is the wrong way round for a live stream: the packets at the front are
//! the ones furthest from being playable in time, and the arriving one is the
//! only part of the stream that is still current. So the buffer preserved a
//! second and a half of stale audio and threw away everything live, and it did
//! so permanently, because a refusal is not a way back down. A real device sat
//! at 1536 ms -- 256 packets of six -- for a whole session that way.
//!
//! Every timeline below is synthetic. This crate has no clock.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, PushOutcome, Transport};

const RATE: u64 = 48_000;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

/// The link these tests run on, and the source of every number asserted
/// against below. Nothing here is a constant of its own.
const LINK: JitterConfig = JitterConfig::for_transport(Transport::Usb);

/// The depth the ceiling alone allows, which is the number the real device
/// reported: 256 packets at 6 ms.
fn hard_ceiling_ms() -> f64 {
    LINK.max_packets as f64 * PACKET_MS
}

/// A packet that says which packet it is, so the audio that comes out can be
/// compared against the audio that went in.
fn packet(sequence: u16) -> Vec<u8> {
    let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
    pcm[..2].copy_from_slice(&sequence.to_le_bytes());
    pcm
}

fn tag(pcm: &[u8]) -> u16 {
    u16::from_le_bytes([pcm[0], pcm[1]])
}

/// A receiver that never applies the millisecond budget, and whose sink has
/// stopped: the shape of every way the ceiling is still reachable.
///
/// Nothing here calls `shed_over_budget` and nothing calls `pop` until the
/// stream is over, which is exactly what the receive loop does while there is
/// no playback queue to measure a budget against or hand audio to.
struct Unbounded {
    buffer: JitterBuffer,
    /// Depth after each arrival, in packets.
    depths: Vec<usize>,
}

impl Unbounded {
    fn new(config: JitterConfig) -> Self {
        Self {
            buffer: JitterBuffer::new(Format::stereo_48k(), config),
            depths: Vec::new(),
        }
    }

    fn receive(&mut self, sequence: u16) -> PushOutcome {
        let outcome = self.buffer.push(
            sequence,
            u32::from(sequence).wrapping_mul(FRAMES_PER_PACKET),
            u64::from(sequence) * PACKET_NANOS,
            packet(sequence),
        );
        self.depths.push(self.buffer.depth_packets());
        outcome
    }

    /// Everything the buffer will release, and every slot it gave up on.
    fn drain(&mut self) -> (Vec<u16>, usize) {
        let mut played = Vec::new();
        let mut concealed = 0;
        loop {
            match self.buffer.pop() {
                PopOutcome::Packet(pcm) => played.push(tag(&pcm)),
                PopOutcome::Lost => concealed += 1,
                PopOutcome::Starved => return (played, concealed),
            }
        }
    }
}

/// A sender that produces one packet every six milliseconds and never varies,
/// into a receiver that never gives anything back.
fn flood(packets: u16) -> Unbounded {
    let mut session = Unbounded::new(LINK);
    for sequence in 0..packets {
        session.receive(sequence);
    }
    session
}

#[test]
fn at_the_ceiling_the_audio_kept_is_the_recent_audio() {
    // Six seconds of audio into a receiver holding a second and a half at
    // most. Four fifths of it cannot be played, and which four fifths is the
    // whole question.
    let sent = 1_000_u16;
    let mut session = flood(sent);

    let sent_ms = f64::from(sent) * PACKET_MS;
    assert!(
        sent_ms > hard_ceiling_ms() * 2.0,
        "the timeline is too short to reach the failure it is about: \
         {sent_ms:.0} ms against a {:.0} ms ceiling",
        hard_ceiling_ms()
    );

    let (played, concealed) = session.drain();

    assert!(!played.is_empty(), "the buffer played nothing at all");
    assert_eq!(concealed, 0, "{concealed} shed slots were concealed");

    // The end of the stream, not the beginning. This is the assertion the old
    // buffer failed: it would have played packets 0 to 255 and refused every
    // one of the 744 that followed.
    let expected: Vec<u16> = (sent - played.len() as u16..sent).collect();
    assert_eq!(
        played,
        expected,
        "the buffer played stale audio: {}..{} of {sent}",
        played[0],
        played[played.len() - 1]
    );
    assert_eq!(
        played.last(),
        Some(&(sent - 1)),
        "the newest packet did not survive"
    );
}

#[test]
fn the_ceiling_never_lets_the_buffer_past_it() {
    let session = flood(1_000);

    let peak = session.depths.iter().copied().max().expect("no arrivals");
    assert!(
        peak <= LINK.max_packets,
        "held {peak} packets against a {} packet ceiling",
        LINK.max_packets
    );
}

#[test]
fn the_depth_returns_to_the_target_rather_than_pinning_at_the_ceiling() {
    // The other half of the old failure. Refusing new audio holds the depth at
    // the ceiling forever, because nothing else in this buffer gives depth
    // back; a discard from the front lands on the target, so one discard buys
    // the whole difference and the depth is only ever near the ceiling on the
    // way to the next one.
    let session = flood(1_000);
    let target_packets = (session.buffer.target_ms() / PACKET_MS).ceil() as usize;

    // Past the initial fill, which is the one stretch that has never been near
    // the ceiling and so proves nothing about what happens at it.
    let floor = session
        .depths
        .iter()
        .copied()
        .skip(LINK.max_packets)
        .min()
        .expect("the timeline never reached the ceiling");

    assert_eq!(
        floor, target_packets,
        "the shallowest the buffer ever got after the ceiling was {floor} \
         packets, against a {target_packets} packet target"
    );
}

#[test]
fn every_packet_is_either_played_or_shed_and_none_is_refused() {
    let sent = 1_000_u16;
    let mut session = flood(sent);
    let (played, concealed) = session.drain();
    let stats = session.buffer.stats();

    // Nothing was turned away: every packet the sender produced was stored.
    assert_eq!(stats.accepted, u64::from(sent), "the buffer refused audio");
    assert_eq!(stats.too_late, 0);
    assert_eq!(stats.duplicates, 0);

    // And every one of them either played or was shed. Nothing went missing,
    // and nothing that was shed was counted as loss.
    assert_eq!(
        played.len() + stats.shed as usize,
        usize::from(sent),
        "{} played and {} shed of {sent}",
        played.len(),
        stats.shed
    );
    assert_eq!(stats.lost, 0, "shed audio was reported as loss");
    assert_eq!(concealed, 0);

    // The event is reported, because reaching it means the budget above it was
    // never applied. It is not reported once per packet: each discard lands on
    // the target, so they are hundreds of packets apart.
    assert!(stats.overflows > 0, "the ceiling was reached silently");
    assert!(
        stats.overflows < u64::from(sent) / 100,
        "the ceiling fired {} times in {sent} packets",
        stats.overflows
    );
}

#[test]
fn a_reordered_straggler_at_the_ceiling_is_not_refused() {
    // The one packet whose arrival can be older than most of what is held. It
    // is judged with the rest of the front rather than turned away, and either
    // way the buffer must not report it late or lose the slot.
    let mut session = Unbounded::new(JitterConfig {
        max_packets: 16,
        ..LINK
    });

    for sequence in 0..16_u16 {
        assert_eq!(session.receive(sequence), PushOutcome::Accepted);
    }
    // 16 was never sent, so 17 leaves a hole the network is still filling.
    session.receive(17);
    session.receive(16);

    let stats = session.buffer.stats();
    assert_eq!(stats.too_late, 0, "the straggler was refused");
    assert_eq!(stats.accepted, 18);
    assert_eq!(stats.reordered, 1);

    let (played, concealed) = session.drain();
    assert_eq!(concealed, 0, "a shed slot was concealed");
    assert!(
        played.windows(2).all(|pair| pair[0] < pair[1]),
        "{played:?}"
    );
    assert_eq!(played.last(), Some(&17));
}

#[test]
fn a_session_that_stays_inside_the_ceiling_behaves_exactly_as_before() {
    // The change must be invisible to everything that was already working.
    // Twenty packets into a 256 packet buffer is every healthy session there
    // has ever been.
    let mut session = Unbounded::new(LINK);
    for sequence in 0..20_u16 {
        assert_eq!(session.receive(sequence), PushOutcome::Accepted);
    }

    let stats = session.buffer.stats();
    assert_eq!(stats.overflows, 0);
    assert_eq!(stats.shed, 0);
    assert_eq!(session.buffer.depth_packets(), 20);

    let (played, concealed) = session.drain();
    assert_eq!(concealed, 0);
    assert_eq!(played, (0..20_u16).collect::<Vec<_>>());
}
