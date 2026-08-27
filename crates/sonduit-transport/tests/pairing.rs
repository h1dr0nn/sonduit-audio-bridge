//! Pairing over real sockets.
//!
//! The unit tests check the tag arithmetic. This checks the thing that would
//! actually go wrong: two devices answer the same probe, one knows the pairing
//! code and one does not, and only the first is offered to the user.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use sonduit_transport::discovery::{
    audio_address, decode_announce, encode_announce, encode_probe, probe_nonce,
};
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};

/// A device that answers probes with `code`.
fn responder(
    code: PairingCode,
    name: &'static str,
    audio_port: u16,
) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let address = socket.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let mut datagram = [0_u8; 256];
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            return;
        };
        let Some(nonce) = probe_nonce(&datagram[..length]) else {
            return;
        };
        let reply = encode_announce(name, audio_port, &nonce, &code);
        let _ = socket.send_to(&reply, from);
    });

    (address, handle)
}

fn nonce(seed: u8) -> [u8; NONCE_BYTES] {
    [seed; NONCE_BYTES]
}

#[test]
fn only_the_device_that_knows_the_code_is_offered() {
    let paired = PairingCode::parse("482913").unwrap();
    let stranger = PairingCode::parse("111111").unwrap();

    let (good_address, good) = responder(paired.clone(), "Pixel 8", 4010);
    let (bad_address, bad) = responder(stranger, "Not yours", 4010);

    let prober = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    prober
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    let scan = nonce(0x5A);
    let probe = encode_probe(&scan);
    prober.send_to(&probe, good_address).unwrap();
    prober.send_to(&probe, bad_address).unwrap();

    let mut accepted = Vec::new();
    let mut replies = 0;
    let mut datagram = [0_u8; 256];

    while replies < 2 {
        let Ok((length, from)) = prober.recv_from(&mut datagram) else {
            break;
        };
        replies += 1;
        if let Some(announcement) = decode_announce(&datagram[..length], &scan, &paired) {
            accepted.push(audio_address(from, &announcement));
        }
    }

    let _ = good.join();
    let _ = bad.join();

    assert_eq!(replies, 2, "both devices answered the probe");
    assert_eq!(
        accepted.len(),
        1,
        "exactly one device should have been accepted"
    );
    assert_eq!(
        accepted[0].ip(),
        good_address.ip(),
        "the accepted device is the paired one"
    );
    assert_eq!(accepted[0].port(), 4010);
}

#[test]
fn a_typo_in_the_code_finds_nothing_rather_than_the_wrong_device() {
    // The failure mode that matters for usability: a mistyped digit must not
    // silently pair with something else.
    let phone = PairingCode::parse("482913").unwrap();
    let typo = PairingCode::parse("482914").unwrap();

    let (address, responder_handle) = responder(phone, "Pixel 8", 4010);

    let prober = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    prober
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    let scan = nonce(0x11);
    prober.send_to(&encode_probe(&scan), address).unwrap();

    let mut datagram = [0_u8; 256];
    let (length, _) = prober.recv_from(&mut datagram).unwrap();
    let _ = responder_handle.join();

    assert_eq!(
        decode_announce(&datagram[..length], &scan, &typo),
        None,
        "a one-digit typo accepted the device anyway"
    );
}

#[test]
fn a_reply_to_an_earlier_probe_is_not_accepted_for_a_later_one() {
    // A device on the network that records one exchange must not be able to
    // impersonate the phone at every future scan.
    let phone = PairingCode::parse("482913").unwrap();
    let (address, responder_handle) = responder(phone.clone(), "Pixel 8", 4010);

    let prober = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    prober
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    let first_scan = nonce(0x22);
    prober.send_to(&encode_probe(&first_scan), address).unwrap();

    let mut datagram = [0_u8; 256];
    let (length, _) = prober.recv_from(&mut datagram).unwrap();
    let captured = datagram[..length].to_vec();
    let _ = responder_handle.join();

    assert!(
        decode_announce(&captured, &first_scan, &phone).is_some(),
        "the reply should be valid for the probe it answered"
    );

    let second_scan = nonce(0x33);
    assert_eq!(
        decode_announce(&captured, &second_scan, &phone),
        None,
        "a captured reply was replayed successfully"
    );
}
