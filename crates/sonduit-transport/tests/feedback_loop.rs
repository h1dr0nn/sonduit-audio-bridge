//! Does the sender learn anything from a receiver that answers?
//!
//! The unit tests check the encoding and the arithmetic. This checks the thing
//! that was actually broken: a sender talking to nobody was reporting a
//! working session, because it had no way to tell the difference. Both cases
//! run over real sockets here, because "no receiver" is the case that used to
//! look identical to success.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use sonduit_transport::feedback::{one_way_ms, Feedback, FEEDBACK_BYTES};
use sonduit_transport::roundtrip::RoundTrip;

/// A receiver that answers one packet with one report, after holding it.
fn responder(hold: Duration) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let address = socket.local_addr().unwrap();

    let handle = std::thread::spawn(move || {
        let mut datagram = [0_u8; 2048];
        let Ok((_, from)) = socket.recv_from(&mut datagram) else {
            return;
        };
        // The echo is the timestamp the sender put in the packet. Here the
        // test writes it as the whole payload for simplicity.
        let echo = u32::from_le_bytes(datagram[..4].try_into().unwrap());
        std::thread::sleep(hold);

        let report = Feedback {
            echo,
            hold_ms: hold.as_millis() as u16,
            accepted: 97,
            lost: 3,
            depth_tenths_ms: 284,
            playing: true,
        };
        let mut out = [0_u8; FEEDBACK_BYTES];
        report.encode(&mut out).unwrap();
        let _ = socket.send_to(&out, from);
    });

    (address, handle)
}

#[test]
fn a_sender_with_no_receiver_measures_nothing() {
    // The bug. Sending into an address nobody is listening on succeeds at
    // every layer the sender can see, and it used to be reported as a working
    // session at a plausible latency with no loss.
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket.set_nonblocking(true).unwrap();

    // Bound and immediately dropped, so the port is real and unoccupied.
    let dead = {
        let probe = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        probe.local_addr().unwrap()
    };

    let mut round_trip = RoundTrip::new();
    let started = Instant::now();

    for timestamp in 0..50_u32 {
        round_trip.record_send(timestamp, started.elapsed().as_nanos() as u64);
        let _ = socket.send_to(&timestamp.to_le_bytes(), dead);
    }

    let mut buffer = [0_u8; FEEDBACK_BYTES];
    let mut reports = 0;
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        if let Ok((length, _)) = socket.recv_from(&mut buffer) {
            if Feedback::decode(&buffer[..length]).is_some() {
                reports += 1;
            }
        }
    }

    assert_eq!(reports, 0, "something answered an address nobody is on");
    assert_eq!(
        round_trip.round_trip_ms(),
        None,
        "a latency was produced with no receiver"
    );
    assert_eq!(round_trip.samples(), 0);
}

#[test]
fn a_receiver_that_answers_is_measured() {
    let (target, responder_handle) = responder(Duration::from_millis(0));

    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();

    let mut round_trip = RoundTrip::new();
    let started = Instant::now();

    let timestamp = 12_345_u32;
    round_trip.record_send(timestamp, started.elapsed().as_nanos() as u64);
    socket.send_to(&timestamp.to_le_bytes(), target).unwrap();

    let mut buffer = [0_u8; FEEDBACK_BYTES];
    let (length, _) = socket.recv_from(&mut buffer).expect("no report arrived");
    let _ = responder_handle.join();

    let report = Feedback::decode(&buffer[..length]).expect("the report should decode");
    assert_eq!(report.echo, timestamp, "the echo names another packet");
    assert_eq!(report.accepted, 97);
    assert_eq!(report.lost, 3);
    assert_eq!(report.depth_ms(), 28.4);
    assert!(report.playing);

    let measured = round_trip
        .observe_echo(report.echo, started.elapsed().as_nanos() as u64)
        .expect("the echo should match the send");

    // Loopback, so this is small but never negative and never absurd.
    assert!(
        (0.0..200.0).contains(&measured),
        "round trip measured as {measured} ms"
    );
    assert!(round_trip.round_trip_ms().is_some());
    assert_eq!(round_trip.samples(), 1);
}

#[test]
fn a_receiver_that_dawdles_is_not_charged_for_the_network() {
    // A receiver batching its reports must not look like a slow link. The
    // hold time it declares comes back out of the round trip.
    let hold = Duration::from_millis(60);
    let (target, responder_handle) = responder(hold);

    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(800)))
        .unwrap();

    let mut round_trip = RoundTrip::new();
    let started = Instant::now();
    let timestamp = 7_u32;
    round_trip.record_send(timestamp, started.elapsed().as_nanos() as u64);
    socket.send_to(&timestamp.to_le_bytes(), target).unwrap();

    let mut buffer = [0_u8; FEEDBACK_BYTES];
    let (length, _) = socket.recv_from(&mut buffer).expect("no report arrived");
    let _ = responder_handle.join();

    let report = Feedback::decode(&buffer[..length]).unwrap();
    let measured = round_trip
        .observe_echo(report.echo, started.elapsed().as_nanos() as u64)
        .unwrap();

    assert!(
        measured >= 55.0,
        "the round trip should include the hold: {measured} ms"
    );
    let network = one_way_ms(measured, report.hold_ms);
    assert!(
        network < 20.0,
        "the receiver's own {} ms was charged to the network: {network} ms one way",
        report.hold_ms
    );
}
