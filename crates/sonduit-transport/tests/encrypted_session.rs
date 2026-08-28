//! An encrypted session, end to end.
//!
//! pairing handshake -> sine source -> seal -> real UDP socket -> open ->
//! jitter buffer -> WAV sink, then assertions on the bytes that came out.
//!
//! The point of driving the real jitter buffer rather than asserting on the
//! cipher alone is that a construction can be perfectly sound and still be
//! wrong for this transport. UDP loses, reorders and duplicates by design, so
//! the interesting question is not "does it decrypt" but "does a stream that
//! loses a packet still recover exactly as it does today". These tests answer
//! that by running the same arrival pattern down both paths and comparing.
//!
//! What they do **not** prove: that anything is audible on an Android device.
//! No audio hardware is involved. See `docs/environment.md`.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, PushOutcome};
use sonduit_core::packet::SonduitPacket;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::sealed::{Opener, SealError, Sealer};
use sonduit_transport::session::{KeyExchange, SessionSecret, SALT_BYTES, SEED_BYTES};
use sonduit_transport::sink::{AudioSink, WavFileSink, WAV_HEADER_BYTES};
use sonduit_transport::source::{AudioSource, SineSource};
use sonduit_transport::{discovery, MAX_DATAGRAM_BYTES};

const PACKETS: u16 = 60;
const SALT: [u8; SALT_BYTES] = [0x3C; SALT_BYTES];

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "sonduit-encrypted-{name}-{}.wav",
        std::process::id()
    ));
    path
}

fn read_i16(bytes: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn config() -> JitterConfig {
    JitterConfig {
        target_ms: 12,
        min_ms: 6,
        max_ms: 200,
        jitter_multiplier: 3.0,
        max_packets: 256,
        ..JitterConfig::default()
    }
}

/// Run the whole pairing handshake and hand back what each end ends up with.
///
/// The real four datagrams rather than a fabricated secret: a test that starts
/// from key material proves nothing about the path a session actually takes,
/// and the handshake is the half of this design that could be got wrong
/// without any test noticing.
fn paired(code_digits: &str, desktop_seed: u8, phone_seed: u8) -> (SessionSecret, SessionSecret) {
    let nonce = [0x5A_u8; NONCE_BYTES];
    let code = PairingCode::parse(code_digits).expect("a six digit code");

    let desktop = KeyExchange::from_seed([desktop_seed; SEED_BYTES]);
    let phone = KeyExchange::from_seed([phone_seed; SEED_BYTES]);
    let (pa, pb) = (desktop.public_key(), phone.public_key());

    let offer = discovery::encode_key_offer(&pa, &nonce, &code);
    let seen_pa = discovery::decode_key_offer(&offer, &nonce, &code).expect("the offer verifies");

    let accept = discovery::encode_key_accept(&pb, &seen_pa, &nonce, &code);
    let seen_pb =
        discovery::decode_key_accept(&accept, &pa, &nonce, &code).expect("the accept verifies");

    let sending = desktop
        .agree(&seen_pb, &nonce, &code, &pa, &seen_pb)
        .expect("a contributory exchange");
    let receiving = phone
        .agree(&seen_pa, &nonce, &code, &seen_pa, &pb)
        .expect("a contributory exchange");

    (sending, receiving)
}

#[test]
fn sine_survives_the_encrypted_chain_and_lands_in_a_wav_file() {
    let format = Format::stereo_48k();
    let frames_per_packet = format.frames_per_packet().unwrap();
    let path = temp_path("full");

    let (sending, receiving) = paired("482913", 11, 22);

    // Receiver first, so the sender has somewhere to send to.
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let destination: SocketAddr = receiver.local_addr().expect("local addr");
    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");

    let mut sealer = Sealer::new(&sending, SALT);
    let mut opener = Opener::new(receiving);

    let mut source = SineSource::new(format, Some(u64::from(PACKETS) * frames_per_packet as u64));
    let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut datagram = vec![0_u8; Sealer::sealed_len(PCM_PAYLOAD_BYTES)];
    let mut scratch = vec![0_u8; MAX_DATAGRAM_BYTES];
    // The receiver's one reusable plaintext buffer. Nothing on this path
    // allocates per packet, which is the constraint the audio thread imposes.
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];

    let mut buffer = JitterBuffer::new(format, config());

    let mut sent = 0_u16;
    let mut arrival_nanos = 0_u64;

    // Drained as it goes, for the reason walking_skeleton.rs gives: a socket
    // receive buffer is finite and the kernel drops the overflow in silence.
    while source.read(&mut pcm) == PCM_PAYLOAD_BYTES {
        sealer
            .seal(
                &format,
                u32::from(sent) * frames_per_packet as u32,
                0,
                &pcm,
                &mut datagram,
            )
            .expect("seal");
        sender.send_to(&datagram, destination).expect("send");
        sent += 1;

        let (length, _) = receiver.recv_from(&mut scratch).expect("recv");
        let received = &scratch[..length];

        assert!(
            Opener::is_sealed(received),
            "a sealed datagram must be recognisable without a key"
        );
        assert_ne!(
            &received[32..32 + PCM_PAYLOAD_BYTES],
            &pcm[..],
            "the audio went out in the clear"
        );

        let opened = opener.open(received, &mut plaintext).expect("open");
        assert_eq!(opened.format, format, "format survived the wire");

        buffer.push(
            opened.sequence,
            opened.timestamp_frames,
            arrival_nanos,
            opened.pcm.to_vec(),
        );
        arrival_nanos += format.packet_duration_nanos().unwrap();
    }

    assert_eq!(sent, PACKETS, "sender produced every packet");
    assert_eq!(opener.rejected(), 0, "a clean link rejected something");

    let mut sink = WavFileSink::create(&path, format).expect("create wav");
    let mut played = 0_u32;
    let mut lost = 0_u32;
    loop {
        match buffer.pop() {
            PopOutcome::Packet(pcm) => {
                sink.write(&pcm).expect("write");
                played += 1;
            }
            PopOutcome::Lost => {
                sink.write(&vec![0_u8; PCM_PAYLOAD_BYTES]).expect("write");
                lost += 1;
            }
            PopOutcome::Starved => break,
        }
    }
    sink.finish().expect("finish");

    assert_eq!(lost, 0, "loopback must not lose packets");
    assert_eq!(played, u32::from(PACKETS), "every packet reached the sink");

    // The samples must be the sine that went in, not silence and not noise.
    let bytes = std::fs::read(&path).expect("read back");
    let expected_data = u32::from(PACKETS) * PCM_PAYLOAD_BYTES as u32;
    assert_eq!(read_u32(&bytes, 40), expected_data, "declared data size");
    assert_eq!(bytes.len(), WAV_HEADER_BYTES + expected_data as usize);

    let reference = SineSource::new(format, None);
    let pcm_bytes = &bytes[WAV_HEADER_BYTES..];
    let total_frames = u64::from(PACKETS) * frames_per_packet as u64;

    let mut peak = 0_i32;
    for frame in 0..total_frames {
        let at = frame as usize * format.bytes_per_frame();
        let left = read_i16(pcm_bytes, at);
        let expected = (reference.sample_at(frame) * f64::from(i16::MAX)) as i16;
        assert_eq!(
            left, expected,
            "sample {frame} does not match the source waveform"
        );
        peak = peak.max(i32::from(left).abs());
    }
    assert!(peak > 16_000, "waveform is present but too quiet: {peak}");

    std::fs::remove_file(&path).ok();
}

#[test]
fn a_tampered_packet_is_refused_and_never_reaches_the_buffer() {
    // The failure that matters: audio an attacker chose being played. Every
    // byte of the datagram is covered, header included, because the header
    // carries the format and the link flag the receiver acts on.
    let format = Format::stereo_48k();
    let (sending, receiving) = paired("482913", 11, 22);

    let mut sealer = Sealer::new(&sending, SALT);
    let mut opener = Opener::new(receiving);
    let buffer = JitterBuffer::new(format, config());

    let pcm: Vec<u8> = (0..PCM_PAYLOAD_BYTES)
        .map(|index| (index % 251) as u8)
        .collect();
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];

    for at in [0_usize, 4, 5, 6, 12, 18, 20, 24, 32, 600, 1183] {
        let mut datagram = vec![0_u8; Sealer::sealed_len(pcm.len())];
        sealer
            .seal(&format, 0, 0, &pcm, &mut datagram)
            .expect("seal");
        datagram[at] ^= 0x01;

        assert!(
            opener.open(&datagram, &mut plaintext).is_err(),
            "byte {at} could be rewritten in flight"
        );
    }

    assert_eq!(
        buffer.stats().accepted,
        0,
        "forged audio reached the buffer"
    );
    assert_eq!(opener.rejected(), 11);
}

#[test]
fn a_packet_from_a_peer_with_the_wrong_key_is_refused() {
    // Somebody on the same network who never paired, sending a well-formed
    // packet to a listening receiver. Under the cleartext format this played.
    let format = Format::stereo_48k();
    let (ours, receiving) = paired("482913", 11, 22);
    let (theirs, _) = paired("000001", 33, 44);

    let pcm = vec![0x7F_u8; PCM_PAYLOAD_BYTES];
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut opener = Opener::new(receiving);

    let mut stranger = Sealer::new(&theirs, SALT);
    let mut forged = vec![0_u8; Sealer::sealed_len(pcm.len())];
    stranger
        .seal(&format, 0, 0, &pcm, &mut forged)
        .expect("seal");
    assert!(matches!(
        opener.open(&forged, &mut plaintext),
        Err(SealError::NotAuthentic)
    ));

    // The genuine sender still works afterwards: a rejection must not poison
    // the session.
    let mut ours_sealer = Sealer::new(&ours, SALT);
    let mut genuine = vec![0_u8; Sealer::sealed_len(pcm.len())];
    ours_sealer
        .seal(&format, 0, 0, &pcm, &mut genuine)
        .expect("seal");
    assert!(opener.open(&genuine, &mut plaintext).is_ok());
}

#[test]
fn a_cleartext_packet_is_refused_once_a_key_exists() {
    // The downgrade. If a keyed receiver still accepted version 1 audio then
    // an attacker would simply send version 1, and the encryption would be
    // decoration. Refused loudly, with the version named.
    let format = Format::stereo_48k();
    let (_, receiving) = paired("482913", 11, 22);
    let mut opener = Opener::new(receiving);

    let pcm = vec![1_u8; PCM_PAYLOAD_BYTES];
    let packet = SonduitPacket {
        format,
        sequence: 0,
        timestamp_frames: 0,
        flags: 0,
        pcm: &pcm,
    };
    let mut datagram = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
    packet.encode(&mut datagram).expect("encode");

    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    assert!(matches!(
        opener.open(&datagram, &mut plaintext),
        Err(SealError::UnsupportedVersion(1))
    ));
    assert_eq!(sonduit_transport::sonduit_version(&datagram), Some(1));
}

/// Drive one arrival pattern through the jitter buffer, sealed or not, and
/// report what came out of it.
struct Run {
    /// Packet ordinal for each pop, `None` where the buffer concealed a gap.
    played: Vec<Option<u16>>,
    accepted: u64,
    lost: u64,
    reordered: u64,
    duplicates: u64,
    /// Datagrams the cipher layer refused before the buffer ever saw them.
    refused: u64,
}

fn run(arrivals: &[usize], sealed: bool) -> Run {
    let format = Format::stereo_48k();
    let frames_per_packet = format.frames_per_packet().unwrap() as u32;
    let packet_nanos = format.packet_duration_nanos().unwrap();

    let (sending, receiving) = paired("482913", 11, 22);
    let mut sealer = Sealer::new(&sending, SALT);
    let mut opener = Opener::new(receiving);

    // Every packet's payload starts with its own ordinal, so what came out can
    // be identified without trusting the header.
    let mut source = SineSource::new(format, None);
    let mut encoded: Vec<Vec<u8>> = Vec::new();
    for sequence in 0..8_u16 {
        let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
        source.read(&mut pcm);
        pcm[0] = sequence as u8;

        let mut datagram = if sealed {
            vec![0_u8; Sealer::sealed_len(PCM_PAYLOAD_BYTES)]
        } else {
            vec![0_u8; SonduitPacket::encoded_len(PCM_PAYLOAD_BYTES)]
        };
        if sealed {
            sealer
                .seal(
                    &format,
                    sequence as u32 * frames_per_packet,
                    0,
                    &pcm,
                    &mut datagram,
                )
                .expect("seal");
        } else {
            SonduitPacket {
                format,
                sequence,
                timestamp_frames: sequence as u32 * frames_per_packet,
                flags: 0,
                pcm: &pcm,
            }
            .encode(&mut datagram)
            .expect("encode");
        }
        encoded.push(datagram);
    }

    let mut buffer = JitterBuffer::new(format, config());
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut refused = 0;

    for (step, &index) in arrivals.iter().enumerate() {
        let arrival = step as u64 * packet_nanos;
        let datagram = &encoded[index];

        let (sequence, timestamp, pcm) = if sealed {
            match opener.open(datagram, &mut plaintext) {
                Ok(opened) => (
                    opened.sequence,
                    opened.timestamp_frames,
                    opened.pcm.to_vec(),
                ),
                Err(_) => {
                    refused += 1;
                    continue;
                }
            }
        } else {
            let decoded = SonduitPacket::decode(datagram).expect("decode");
            (
                decoded.sequence,
                decoded.timestamp_frames,
                decoded.pcm.to_vec(),
            )
        };

        let outcome = buffer.push(sequence, timestamp, arrival, pcm);
        assert!(
            !matches!(outcome, PushOutcome::Overflow),
            "the buffer overflowed, which this pattern must not cause"
        );
    }

    let mut played = Vec::new();
    loop {
        match buffer.pop() {
            PopOutcome::Packet(pcm) => played.push(Some(u16::from(pcm[0]))),
            PopOutcome::Lost => played.push(None),
            PopOutcome::Starved => break,
        }
    }

    let stats = buffer.stats();
    Run {
        played,
        accepted: stats.accepted,
        lost: stats.lost,
        reordered: stats.reordered,
        duplicates: stats.duplicates,
        refused,
    }
}

#[test]
fn loss_reordering_and_duplication_behave_the_same_encrypted_as_in_the_clear() {
    // Arrival order 0, 2, 1, 4, 4, 3, 6, 7: packet 5 never arrives, 1 and 3
    // arrive late, and 4 arrives twice. This is the pattern the cleartext
    // skeleton test uses, with a duplicate added, because a duplicate is the
    // one case the cipher layer answers before the buffer does.
    let arrivals = [0_usize, 2, 1, 4, 4, 3, 6, 7];

    let clear = run(&arrivals, false);
    let sealed = run(&arrivals, true);

    assert_eq!(
        sealed.played, clear.played,
        "the encrypted stream played different audio"
    );
    assert_eq!(sealed.played.len(), 8);
    assert_eq!(sealed.played[5], None, "the missing packet was concealed");
    assert_eq!(sealed.lost, clear.lost, "loss counted differently");
    assert_eq!(sealed.reordered, clear.reordered, "reordering differed");
    assert_eq!(sealed.lost, 1);
    assert_eq!(sealed.reordered, 2);

    // The one deliberate difference, and it is the safe direction. In the
    // clear, the duplicate reaches the jitter buffer and is counted there.
    // Sealed, the replay window catches it first, so the buffer never sees it
    // and its own duplicate counter stays at zero. The audio is identical
    // either way.
    assert_eq!(clear.duplicates, 1, "the buffer caught the duplicate");
    assert_eq!(sealed.duplicates, 0);
    assert_eq!(sealed.refused, 1, "the replay window caught the duplicate");
    assert_eq!(clear.refused, 0);
    assert_eq!(
        sealed.accepted, clear.accepted,
        "the same seven distinct packets were accepted either way"
    );
}

#[test]
fn a_receiver_that_joins_a_stream_late_needs_no_resynchronisation() {
    // Nothing about the nonce is negotiated: it is the counter, and the salt
    // that picks the key is in every datagram. So a receiver started in the
    // middle of a stream is in step on the first packet it sees, with no
    // handshake and no window to catch up.
    let format = Format::stereo_48k();
    let (sending, receiving) = paired("482913", 11, 22);

    let mut sealer = Sealer::new(&sending, SALT);
    let pcm = vec![0x33_u8; PCM_PAYLOAD_BYTES];
    let mut datagram = vec![0_u8; Sealer::sealed_len(pcm.len())];

    // A little over a sequence-number wrap, so the counter's high half is not
    // zero when the receiver arrives.
    for _ in 0..=(u32::from(u16::MAX) + 5) {
        sealer
            .seal(&format, 0, 0, &pcm, &mut datagram)
            .expect("seal");
    }

    let mut opener = Opener::new(receiving);
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    let opened = opener.open(&datagram, &mut plaintext).expect("open");

    assert_eq!(opened.pcm, &pcm[..]);
    assert!(opened.counter > u64::from(u16::MAX), "no wrap was crossed");
    assert_eq!(opened.sequence, (opened.counter & 0xFFFF) as u16);
}
