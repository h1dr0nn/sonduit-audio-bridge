//! The walking skeleton, end to end.
//!
//! sine source -> core encode -> real UDP socket -> core decode -> jitter
//! buffer -> WAV sink, then assertions on the bytes that came out the far end.
//!
//! This is not a mock. The datagrams cross a real loopback socket and the
//! output is a real file parsed back off disk.
//!
//! What it does **not** prove: that anything is audible on an Android device.
//! No audio hardware is involved anywhere. See `docs/environment.md`.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome};
use sonduit_core::packet::SonduitPacket;
use sonduit_transport::sink::{AudioSink, WavFileSink, WAV_HEADER_BYTES};
use sonduit_transport::source::{AudioSource, SineSource};
use sonduit_transport::{classify, Wire, MAX_DATAGRAM_BYTES};

const PACKETS: u16 = 60;

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "sonduit-skeleton-{name}-{}.wav",
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

#[test]
fn sine_survives_the_whole_chain_and_lands_in_a_wav_file() {
    let format = Format::stereo_48k();
    let frames_per_packet = format.frames_per_packet().unwrap();
    let path = temp_path("full");

    // Receiver first, so the sender has somewhere to send to.
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let destination: SocketAddr = receiver.local_addr().expect("local addr");

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");

    // --- send and receive, interleaved ---------------------------------
    //
    // Deliberately not "send everything, then read everything". A socket
    // receive buffer is finite: at 1172 bytes a datagram, 60 packets is 70 KB,
    // which overruns the default and the kernel silently drops the overflow.
    // A real pipeline drains as it goes, and so does this test.
    let mut source = SineSource::new(format, Some(u64::from(PACKETS) * frames_per_packet as u64));
    let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut datagram = vec![0_u8; SonduitPacket::encoded_len(PCM_PAYLOAD_BYTES)];
    let mut scratch = vec![0_u8; MAX_DATAGRAM_BYTES];

    let mut buffer = JitterBuffer::new(
        format,
        JitterConfig {
            target_ms: 12,
            min_ms: 6,
            max_ms: 200,
            jitter_multiplier: 3.0,
            max_packets: 256,
        },
    );

    let mut sent = 0_u16;
    let mut arrival_nanos = 0_u64;

    while source.read(&mut pcm) == PCM_PAYLOAD_BYTES {
        let packet = SonduitPacket {
            format,
            sequence: sent,
            timestamp_frames: u32::from(sent) * frames_per_packet as u32,
            flags: 0,
            pcm: &pcm,
        };
        packet.encode(&mut datagram).expect("encode");
        sender.send_to(&datagram, destination).expect("send");
        sent += 1;

        let (length, _) = receiver.recv_from(&mut scratch).expect("recv");
        let received = &scratch[..length];

        assert_eq!(classify(received), Some(Wire::Sonduit));
        let decoded = SonduitPacket::decode(received).expect("decode");
        assert_eq!(decoded.format, format, "format survived the wire");

        buffer.push(
            decoded.sequence,
            decoded.timestamp_frames,
            arrival_nanos,
            decoded.pcm.to_vec(),
        );
        arrival_nanos += format.packet_duration_nanos().unwrap();
    }

    assert_eq!(sent, PACKETS, "sender produced every packet");

    // --- play into the sink -----------------------------------------------
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
                // Conceal with silence, which is what a real sink must do.
                sink.write(&vec![0_u8; PCM_PAYLOAD_BYTES]).expect("write");
                lost += 1;
            }
            PopOutcome::Starved => break,
        }
    }
    sink.finish().expect("finish");

    assert_eq!(lost, 0, "loopback must not lose packets");
    assert_eq!(played, u32::from(PACKETS), "every packet reached the sink");

    let stats = buffer.stats();
    assert_eq!(stats.accepted, u64::from(PACKETS));
    assert_eq!(stats.duplicates, 0);
    assert_eq!(stats.too_late, 0);
    assert_eq!(stats.overflows, 0);

    // --- assert on the file itself ----------------------------------------
    let bytes = std::fs::read(&path).expect("read back");
    let expected_data = u32::from(PACKETS) * PCM_PAYLOAD_BYTES as u32;

    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(read_u32(&bytes, 24), 48_000, "sample rate in the header");
    assert_eq!(read_u32(&bytes, 40), expected_data, "declared data size");
    assert_eq!(bytes.len(), WAV_HEADER_BYTES + expected_data as usize);

    // The samples must be the sine that went in, not silence and not noise.
    let reference = SineSource::new(format, None);
    let pcm_bytes = &bytes[WAV_HEADER_BYTES..];
    let total_frames = u64::from(PACKETS) * frames_per_packet as u64;

    let mut peak = 0_i32;
    for frame in 0..total_frames {
        let at = frame as usize * format.bytes_per_frame();
        let left = read_i16(pcm_bytes, at);
        let right = read_i16(pcm_bytes, at + 2);

        assert_eq!(left, right, "stereo channels diverged at frame {frame}");

        let expected = (reference.sample_at(frame) * f64::from(i16::MAX)) as i16;
        assert_eq!(
            left, expected,
            "sample {frame} does not match the source waveform"
        );

        peak = peak.max(i32::from(left).abs());
    }

    // Amplitude 0.5 of full scale, so the peak should be near 16383.
    assert!(
        peak > 16_000,
        "waveform is present but too quiet, peak {peak}"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn the_chain_reorders_and_conceals_when_the_network_misbehaves() {
    // Same chain, but the datagrams are handed to the buffer out of order with
    // one missing, which is what a real link does.
    let format = Format::stereo_48k();
    let frames_per_packet = format.frames_per_packet().unwrap();
    let path = temp_path("lossy");

    let mut source = SineSource::new(format, None);
    let mut encoded: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];

    for sequence in 0..8_u16 {
        source.read(&mut pcm);
        encoded.push((sequence, pcm.clone()));
    }

    let mut buffer = JitterBuffer::new(
        format,
        JitterConfig {
            target_ms: 12,
            min_ms: 6,
            max_ms: 200,
            jitter_multiplier: 3.0,
            max_packets: 256,
        },
    );

    // Arrival order: 0, 2, 1, 4, 3, 6, 7. Packet 5 never arrives.
    for index in [0_usize, 2, 1, 4, 3, 6, 7] {
        let (sequence, pcm) = &encoded[index];
        buffer.push(
            *sequence,
            u32::from(*sequence) * frames_per_packet as u32,
            u64::from(*sequence) * format.packet_duration_nanos().unwrap(),
            pcm.clone(),
        );
    }

    let mut sink = WavFileSink::create(&path, format).expect("create wav");
    let mut order = Vec::new();
    loop {
        match buffer.pop() {
            PopOutcome::Packet(pcm) => {
                sink.write(&pcm).expect("write");
                order.push(Some(pcm[0]));
            }
            PopOutcome::Lost => {
                sink.write(&vec![0_u8; PCM_PAYLOAD_BYTES]).expect("write");
                order.push(None);
            }
            PopOutcome::Starved => break,
        }
    }
    sink.finish().expect("finish");

    // Seven packets plus one concealed gap where packet 5 should have been.
    assert_eq!(order.len(), 8, "got {order:?}");
    assert_eq!(order[5], None, "the missing packet was concealed");
    assert_eq!(buffer.stats().lost, 1);
    assert_eq!(buffer.stats().reordered, 2, "packets 1 and 3 arrived late");

    let bytes = std::fs::read(&path).expect("read back");
    assert_eq!(read_u32(&bytes, 40), 8 * PCM_PAYLOAD_BYTES as u32);

    // The concealed packet must be actual silence.
    let gap_at = WAV_HEADER_BYTES + 5 * PCM_PAYLOAD_BYTES;
    assert!(
        bytes[gap_at..gap_at + PCM_PAYLOAD_BYTES]
            .iter()
            .all(|byte| *byte == 0),
        "concealment must write silence"
    );

    std::fs::remove_file(&path).ok();
}
