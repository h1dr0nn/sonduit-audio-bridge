//! A paired session, from the first probe to the audio, over real sockets.
//!
//! `encrypted_session.rs` proves the cipher and the buffer behave; this proves
//! the two ends can get to a key at all. The difference matters, because the
//! handshake is the part that has to agree across two processes on two
//! machines, and a test that starts from an already-agreed secret cannot see
//! it disagree.
//!
//! So nothing here is fabricated. A thread stands in for the phone -- it
//! answers probes the way `sonduit-ffi`'s responder does and answers key
//! offers the way it does -- and everything crosses a loopback UDP socket:
//!
//! ```text
//!   probe -> announce -> key offer -> key accept -> sealed audio -> sealed report
//! ```
//!
//! What it does **not** prove: anything about audio hardware, or about a real
//! network. See `docs/environment.md`.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::packet::SonduitPacket;
use sonduit_transport::feedback::Feedback;
use sonduit_transport::handshake::{self, Offer};
use sonduit_transport::packetize::Packetizer;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::sealed::{
    FeedbackOpener, FeedbackSealer, Opener, SealError, Sealer, SEALED_FEEDBACK_BYTES,
};
use sonduit_transport::session::{SessionSecret, SALT_BYTES, SEED_BYTES};
use sonduit_transport::source::{AudioSource, SineSource};
use sonduit_transport::{discovery, TransportError, Wire, MAX_DATAGRAM_BYTES};

/// Packets the streaming tests send.
const PACKETS: usize = 24;

/// How long a step of the handshake waits before the test gives up.
const STEP: Duration = Duration::from_secs(3);

/// The phone's side of pairing, as `sonduit-ffi`'s responder runs it.
///
/// One socket, one thread, for as long as the guard lives. It answers probes
/// with an announcement tagged by `code`, and answers the key offer that
/// follows with its own public key, keeping the secret.
struct StandInPhone {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Option<SessionSecret>>>,
}

impl StandInPhone {
    fn spawn(name: &'static str, audio_port: u16, code: PairingCode) -> Self {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .expect("a fresh socket takes a timeout");
        let port = socket.local_addr().expect("bound").port();

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut datagram = [0_u8; 256];
                // The nonces of probes answered, exactly as the responder
                // keeps them: a key offer names none of them and the tag is
                // what decides which it belongs to.
                let mut recent: Vec<[u8; NONCE_BYTES]> = Vec::new();
                let mut secret = None;

                while !stop.load(Ordering::Relaxed) {
                    let Ok((length, from)) = socket.recv_from(&mut datagram) else {
                        continue;
                    };
                    let bytes = &datagram[..length];

                    if handshake::is_key_offer(bytes) {
                        for nonce in recent.iter().rev() {
                            if let Some((accept, agreed)) =
                                handshake::answer(bytes, nonce, &code, [0x2B; SEED_BYTES])
                            {
                                let _ = socket.send_to(&accept, from);
                                secret = Some(agreed);
                                break;
                            }
                        }
                        continue;
                    }

                    let Some(nonce) = discovery::probe_nonce(bytes) else {
                        continue;
                    };
                    if !recent.contains(&nonce) {
                        recent.push(nonce);
                    }
                    let reply = discovery::encode_announce(name, audio_port, &nonce, &code);
                    let _ = socket.send_to(&reply, from);
                }

                secret
            })
        };

        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    /// Stop answering and hand back whatever the phone ended up holding.
    fn finish(mut self) -> Option<SessionSecret> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().and_then(|thread| thread.join().ok())?
    }
}

impl Drop for StandInPhone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The desktop's side of pairing: probe, verify, offer a key, take the accept.
///
/// The same order `bridge::discover` uses, over the same datagrams.
fn pair_with(phone: &StandInPhone, code: &PairingCode) -> Option<(SocketAddr, SessionSecret)> {
    let nonce = [0x5A_u8; NONCE_BYTES];
    let desktop = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).ok()?;
    desktop
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, phone.port));
    let mut datagram = [0_u8; 256];

    let mut responder = None;
    let deadline = Instant::now() + STEP;
    while Instant::now() < deadline && responder.is_none() {
        let _ = desktop.send_to(&discovery::encode_probe(&nonce), target);
        let Ok((length, from)) = desktop.recv_from(&mut datagram) else {
            continue;
        };
        if discovery::decode_announce(&datagram[..length], &nonce, code).is_some() {
            responder = Some(from);
        }
    }
    let responder = responder?;

    let offer = Offer::new([0x1D; SEED_BYTES], nonce, code.clone());
    let mut accept = None;
    let deadline = Instant::now() + STEP;
    while Instant::now() < deadline && accept.is_none() {
        let _ = desktop.send_to(&offer.datagram(), responder);
        let Ok((length, _)) = desktop.recv_from(&mut datagram) else {
            continue;
        };
        if offer.is_our_accept(&datagram[..length]) {
            accept = Some(datagram[..length].to_vec());
        }
    }

    offer.accept(&accept?).map(|secret| (responder, secret))
}

/// Whether `haystack` contains `needle` anywhere in it.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[test]
fn a_paired_session_streams_audio_whose_bytes_on_the_wire_are_not_the_audio() {
    // The whole point of ADR-009 in one assertion: the PCM the sender captured
    // does not appear in the datagrams it sends, and the receiver still gets
    // exactly that PCM back out.
    let format = Format::stereo_48k();
    let code = PairingCode::parse("482913").expect("a six digit code");

    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let audio_port = receiver.local_addr().expect("bound").port();

    let phone = StandInPhone::spawn("Pixel 7a", audio_port, code.clone());
    let (_, desktop_secret) = pair_with(&phone, &code).expect("the pairing must complete");
    let phone_secret = phone.finish().expect("the phone must have kept a secret");

    // Both ends derived the same thing from four datagrams they exchanged.
    let salt = [0x9F_u8; SALT_BYTES];
    assert_eq!(
        desktop_secret.audio_key(&salt).as_bytes(),
        phone_secret.audio_key(&salt).as_bytes(),
        "the two ends did not agree a key"
    );

    // The sender is the real packetizer, not a hand-driven sealer: this is the
    // call site, and a per-packet copy introduced here would not show up in a
    // test that drove the cipher directly.
    let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");
    let destination = SocketAddr::from((Ipv4Addr::LOCALHOST, audio_port));
    let mut packetizer = Packetizer::sealed(format, Sealer::new(&desktop_secret, salt));
    assert!(packetizer.is_sealed());

    let frames_per_packet = format
        .frames_per_packet()
        .expect("a whole number of frames");
    let mut source = SineSource::new(format, Some(PACKETS as u64 * frames_per_packet as u64));
    let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut opener = Opener::new(phone_secret);
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut scratch = vec![0_u8; MAX_DATAGRAM_BYTES];

    let mut sent_pcm: Vec<Vec<u8>> = Vec::with_capacity(PACKETS);
    let mut received_pcm: Vec<Vec<u8>> = Vec::with_capacity(PACKETS);

    // Drained packet by packet: a socket receive buffer is finite and the
    // kernel drops the overflow without saying so.
    while source.read(&mut pcm) == PCM_PAYLOAD_BYTES {
        sent_pcm.push(pcm.clone());
        packetizer
            .push(&pcm, |datagram| {
                // Nothing the microphone heard is on the wire. Checked against
                // the whole datagram rather than at a fixed offset, so a future
                // header change cannot quietly move the payload into view.
                assert!(
                    !contains(datagram, &pcm[..64]),
                    "a run of the captured audio appeared in the datagram"
                );
                sender
                    .send_to(datagram, destination)
                    .map(|_| ())
                    .map_err(TransportError::from)
            })
            .expect("sending cannot fail on loopback");

        let (length, _) = receiver.recv_from(&mut scratch).expect("recv");
        let opened = opener
            .open(&scratch[..length], &mut plaintext)
            .expect("the receiver holds the key this was sealed under");
        assert_eq!(opened.format, format);
        received_pcm.push(opened.pcm.to_vec());
    }

    assert_eq!(sent_pcm.len(), PACKETS, "the source produced every packet");
    assert_eq!(received_pcm, sent_pcm, "the audio did not survive the wire");
    assert_eq!(opener.rejected(), 0, "a clean loopback rejected something");
}

#[test]
fn the_reports_coming_back_are_sealed_too_and_a_forged_one_is_refused() {
    // The reverse direction is a control channel and it can be forged: a
    // report drives the loss figure, the buffer depth and the round trip the
    // user is shown, and a forged one can keep a dead session looking alive.
    let code = PairingCode::parse("482913").expect("a six digit code");
    let phone = StandInPhone::spawn("Pixel 7a", 4010, code.clone());
    let (_, desktop_secret) = pair_with(&phone, &code).expect("the pairing must complete");
    let phone_secret = phone.finish().expect("the phone kept a secret");

    let desktop = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");
    desktop
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    let to = desktop.local_addr().expect("bound");
    let phone_socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");

    let report = Feedback {
        echo: 12_345,
        hold_ms: 4,
        accepted: 900,
        lost: 1,
        depth_tenths_ms: 421,
        queue_tenths_ms: Some(110),
        playing: true,
    };

    let mut sealer = FeedbackSealer::new(&phone_secret, [0x31; SALT_BYTES]);
    let mut buffer = [0_u8; SEALED_FEEDBACK_BYTES];
    let length = sealer.seal(&report, &mut buffer).expect("seal the report");
    phone_socket.send_to(&buffer[..length], to).expect("send");

    let mut opener = FeedbackOpener::new(desktop_secret);
    let mut arrived = [0_u8; SEALED_FEEDBACK_BYTES];
    let (length, _) = desktop.recv_from(&mut arrived).expect("recv");
    let opened = opener.open(&arrived[..length]).expect("the report opens");
    assert_eq!(opened, report, "the report changed on the way");

    // The same report in the clear, which is what an attacker sends when it
    // has no key. A sender that accepted it would be showing numbers about a
    // session it is not running.
    // Refused on the length before the version is even reached -- a version 1
    // report is 34 bytes and a sealed one is 72 -- which is why the assertion
    // is that it was refused rather than which check caught it. Both are the
    // same answer to the caller: the report is not one this sender may use.
    let mut plain = [0_u8; SEALED_FEEDBACK_BYTES];
    let length = report.encode(&mut plain).expect("encode version 1");
    assert!(
        matches!(
            opener.open(&plain[..length]),
            Err(SealError::TooShort(_) | SealError::UnsupportedVersion(1))
        ),
        "a keyed sender accepted a cleartext report"
    );
    assert_eq!(opener.rejected(), 1);

    // And a report of the right length that was not sealed under this key.
    // A salt of its own, so this is refused by the tag and not by the replay
    // window: reusing the salt above would test the window instead of the key.
    let mut stranger = FeedbackSealer::new(&phone_secret_twin(), [0x32; SALT_BYTES]);
    let length = stranger
        .seal(&report, &mut plain)
        .expect("seal under the wrong key");
    assert!(
        matches!(opener.open(&plain[..length]), Err(SealError::NotAuthentic)),
        "a report from another pairing authenticated"
    );
    assert_eq!(opener.rejected(), 2);
}

/// A master secret from a pairing this session has nothing to do with.
fn phone_secret_twin() -> SessionSecret {
    let nonce = [0x77_u8; NONCE_BYTES];
    let code = PairingCode::parse("000001").expect("six digits");
    let offer = Offer::new([0x40; SEED_BYTES], nonce, code.clone());
    let (accept, _) = handshake::answer(&offer.datagram(), &nonce, &code, [0x41; SEED_BYTES])
        .expect("well formed");
    offer.accept(&accept).expect("a complete agreement")
}

#[test]
fn a_device_that_does_not_hold_the_code_never_reaches_a_key() {
    // The pairing code is twenty bits and it is not the key, but it is what
    // authenticates the exchange that makes one. A device with the wrong code
    // must end the handshake with nothing rather than with a key of its own.
    let phone = StandInPhone::spawn(
        "Stranger",
        4010,
        PairingCode::parse("000001").expect("six digits"),
    );
    let mine = PairingCode::parse("482913").expect("six digits");

    assert!(
        pair_with(&phone, &mine).is_none(),
        "a device with the wrong code was paired with"
    );
    assert!(
        phone.finish().is_none(),
        "the device agreed a key it had no right to"
    );
}

#[test]
fn a_keyed_receiver_refuses_the_cleartext_wire_and_an_unkeyed_one_refuses_the_sealed_wire() {
    // Both refusals, stated as one property: what decides whether audio is
    // played is the key, and neither answer is ever "play it anyway".
    let format = Format::stereo_48k();
    let code = PairingCode::parse("482913").expect("six digits");
    let phone = StandInPhone::spawn("Pixel 7a", 4010, code.clone());
    let (_, desktop_secret) = pair_with(&phone, &code).expect("the pairing must complete");
    let phone_secret = phone.finish().expect("the phone kept a secret");

    let pcm = vec![3_u8; PCM_PAYLOAD_BYTES];
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];

    // A cleartext packet, offered to a receiver that holds a key.
    let mut cleartext = Vec::new();
    Packetizer::new(format, Wire::Sonduit)
        .push(&pcm, |datagram| {
            cleartext = datagram.to_vec();
            Ok(())
        })
        .expect("encoding cannot fail");

    let mut keyed = Opener::new(phone_secret);
    assert!(
        matches!(
            keyed.open(&cleartext, &mut plaintext),
            Err(SealError::UnsupportedVersion(1))
        ),
        "a keyed receiver accepted cleartext, so the encryption is optional"
    );

    // And a sealed packet reaching a receiver with no key at all. There is no
    // Opener to offer it to, which is the point: the refusal is structural.
    let mut sealed = Vec::new();
    Packetizer::sealed(format, Sealer::new(&desktop_secret, [0x5C; SALT_BYTES]))
        .push(&pcm, |datagram| {
            sealed = datagram.to_vec();
            Ok(())
        })
        .expect("sealing cannot fail");

    // An unkeyed receiver reads the version byte and stops there, before a
    // byte of payload is looked at. Decoding it as version 1 audio would play
    // ciphertext at full scale.
    assert!(
        SonduitPacket::decode(&sealed).is_err(),
        "an unkeyed receiver decoded ciphertext as audio"
    );
    assert_eq!(
        sonduit_transport::sonduit_version(&sealed),
        Some(sonduit_core::packet::SONDUIT_VERSION_SEALED)
    );
}
