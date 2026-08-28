//! Proving that the device on the other path is the same phone.
//!
//! # Why this is the hard part
//!
//! Migrating a session means pointing the audio somewhere new. A tether
//! adapter appearing is evidence that *a* phone is plugged in, and nothing
//! more. If the session moves onto it because the timing was suggestive, the
//! machine's entire audio output goes to a device the user never paired --
//! a housemate's phone charging off the same laptop is enough.
//!
//! So availability is never the test. The test is the one the device already
//! passed to be selected at all: it has to answer a fresh discovery probe with
//! a tag keyed by the pairing code, over the path being considered.
//!
//! # What that proves, exactly
//!
//! [`sonduit_transport::discovery::decode_announce`] verifies an HMAC over the
//! announced port and name, keyed by the six-digit pairing code and bound to a
//! nonce minted for this probe. A device that does not hold the code cannot
//! produce the tag, and a recording of an earlier announcement will not verify
//! against a nonce it has never seen. The code is a secret this desktop
//! generated and displayed on its own screen for one pairing, so possession of
//! it is possession of the pairing.
//!
//! The name and port are compared as well. That is not security -- both are
//! inside the tag -- but it costs two comparisons and it catches the one case
//! the tag alone does not: a second device paired with the same code inside
//! the same session.
//!
//! # What it does not prove
//!
//! Nothing about a session with no credential. A user who typed an address by
//! hand, or who is sending to the multicast group, has never given this code
//! anything to check, so migration is switched off for that session rather
//! than guessed at. Declining to move is always safe; moving to a stranger is
//! not.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use sonduit_transport::discovery::{self, Announcement};
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};

use super::link::{LinkKind, Route};

/// How long a verification listens before giving up.
///
/// Short: over the cable a reply is back in a millisecond or two, and over
/// Wi-Fi the probe is repeated rather than waited on. This runs on the watcher
/// thread, so the cost of being wrong about it is a thread that polls late.
const REPLY_WINDOW: Duration = Duration::from_millis(300);

/// How long one blocking read waits, so the window above is actually honoured.
const READ_SLICE: Duration = Duration::from_millis(50);

/// How many probes one verification sends.
///
/// A single broadcast after an idle period is regularly dropped on Wi-Fi, and
/// the tether adapter sits on the Public firewall profile where some drivers
/// drop broadcast outright. Three costs nothing and removes the flake.
const PROBES: u32 = 3;

/// A device this session has authenticated, and the credential that did it.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Where it answered from when it was paired or scanned.
    pub address: SocketAddr,
    /// The name it announced. Covered by the tag, so it cannot be rewritten.
    pub name: String,
    /// The pairing code that proved it. This is the credential every later
    /// check is made against.
    pub code: PairingCode,
}

impl Peer {
    /// The port the peer listens for audio on.
    #[must_use]
    pub const fn audio_port(&self) -> u16 {
        self.address.port()
    }
}

/// Whether an announcement that already verified cryptographically is the peer
/// this session is streaming to, reached where we asked.
///
/// Pure, so the acceptance rule can be driven from a test rather than from a
/// phone. The caller is responsible for having run `decode_announce` first:
/// this adds the identity checks on top of the proof, it does not replace it.
#[must_use]
pub fn is_the_same_device(
    peer: &Peer,
    asked: IpAddr,
    from: SocketAddr,
    announcement: &Announcement,
) -> bool {
    // Answered from somewhere other than where the probe went. Over a tether
    // that is the whole link, so it can only be a second device on the same
    // segment answering, and it is not the one being considered.
    if from.ip() != asked {
        return false;
    }
    // The audio port is where the stream would go. A device that has moved its
    // listener is not something to migrate onto silently.
    if announcement.audio_port != peer.audio_port() {
        return false;
    }
    announcement.name == peer.name
}

/// Ask the device at `route` to prove it is `peer`, over that route.
///
/// The socket is bound to the route's local address, so the probe leaves by
/// the interface being tested and not by whichever one the routing table
/// prefers. That is the same reason the audio socket binds explicitly, and
/// without it this would verify the phone over Wi-Fi and then declare the
/// cable good.
///
/// Returns the route unchanged on success. The target is not taken from the
/// reply: it was addressed to the gateway and only the gateway may answer, so
/// there is nothing new to learn and nothing an answer could redirect.
#[must_use]
pub fn verify(route: &Route, peer: &Peer, nonce: &[u8; NONCE_BYTES]) -> Option<Route> {
    verify_at(route, peer, nonce, discovery::DISCOVERY_PORT)
}

/// [`verify`], with the discovery port named rather than assumed.
///
/// The port is a protocol constant, so nothing in the application varies it.
/// A test does: a stand-in receiver cannot bind the real one without colliding
/// with a live phone on the same machine.
#[must_use]
fn verify_at(route: &Route, peer: &Peer, nonce: &[u8; NONCE_BYTES], port: u16) -> Option<Route> {
    let asked = route.target.ip();
    let socket = UdpSocket::bind(SocketAddr::new(route.bind.ip(), 0)).ok()?;
    socket.set_read_timeout(Some(READ_SLICE)).ok()?;

    if !probe(&socket, SocketAddr::new(asked, port), nonce) {
        return None;
    }

    listen(&socket, peer, nonce, asked, |_| true).map(|_| route.clone())
}

/// Find the peer somewhere that is not a cable.
///
/// Broadcast, because the phone's address on the wireless network is not
/// something this machine can look up: it changes with the DHCP lease and a
/// session that started over USB may never have been told it. Answers that
/// classify as wired are rejected, since the point of asking is to have
/// somewhere to retreat to when the cable goes.
///
/// `classify` is passed in rather than called, so the caller supplies the
/// adapter list it already walked this poll instead of walking it again, and
/// so this stays testable without one.
#[must_use]
pub fn find_elsewhere<F>(peer: &Peer, nonce: &[u8; NONCE_BYTES], classify: F) -> Option<Route>
where
    F: Fn(SocketAddr) -> LinkKind,
{
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket.set_broadcast(true).ok()?;
    socket.set_read_timeout(Some(READ_SLICE)).ok()?;

    if !probe(
        &socket,
        SocketAddr::from((Ipv4Addr::BROADCAST, discovery::DISCOVERY_PORT)),
        nonce,
    ) {
        return None;
    }

    // A tethered phone is on both segments and answers twice. Rejecting the
    // wired reply rather than taking the first one is what stops the fallback
    // being a second name for the link that has just failed.
    //
    // No `asked` address to hold an answer to, because a broadcast was not
    // addressed to anybody; the pairing tag is what stands in for it, and it
    // is the check that was doing the work in the first place.
    let found = listen(
        &socket,
        peer,
        nonce,
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        |from| classify(from) != LinkKind::Wired,
    )?;

    Some(Route::unbound(
        SocketAddr::new(found.0.ip(), found.1.audio_port),
        LinkKind::Wireless,
    ))
}

/// Send a probe several times, reporting whether any of them left the machine.
///
/// False means nothing was even attempted -- an interface that has gone, or a
/// firewall refusing outright -- which is not the same as a probe that went
/// unanswered, and only the second is worth waiting on.
fn probe(socket: &UdpSocket, to: SocketAddr, nonce: &[u8; NONCE_BYTES]) -> bool {
    let datagram = discovery::encode_probe(nonce);
    let mut sent = false;
    for _ in 0..PROBES {
        sent |= socket.send_to(&datagram, to).is_ok();
    }
    sent
}

/// Collect replies until one verifies as `peer` and `accept` agrees.
///
/// Anything that does not verify is dropped in silence, exactly as a scan
/// drops it: it is either a stray datagram on a port Scream also uses or a
/// device that must not be offered, and distinguishing them out loud would
/// make the second look like the first.
///
/// `asked` is the address the probe went to, or `0.0.0.0` when it was a
/// broadcast and there is no single address an answer has to match.
fn listen<F>(
    socket: &UdpSocket,
    peer: &Peer,
    nonce: &[u8; NONCE_BYTES],
    asked: IpAddr,
    mut accept: F,
) -> Option<(SocketAddr, Announcement)>
where
    F: FnMut(SocketAddr) -> bool,
{
    let deadline = Instant::now() + REPLY_WINDOW;
    let mut datagram = [0_u8; 256];

    while Instant::now() < deadline {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        let Some(announcement) = discovery::decode_announce(&datagram[..length], nonce, &peer.code)
        else {
            continue;
        };
        if !accept(from) {
            continue;
        }
        let expected = if asked.is_unspecified() {
            from.ip()
        } else {
            asked
        };
        if !is_the_same_device(peer, expected, from, &announcement) {
            continue;
        }
        return Some((from, announcement));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> Peer {
        Peer {
            address: "192.168.1.42:4010".parse().unwrap(),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
        }
    }

    fn announcement(name: &str, port: u16) -> Announcement {
        Announcement {
            name: name.to_string(),
            audio_port: port,
        }
    }

    fn ip(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    fn from(address: &str) -> SocketAddr {
        address.parse().unwrap()
    }

    #[test]
    fn the_phone_answering_over_the_cable_is_recognised_as_the_same_device() {
        // The whole point: the same phone, a different address, because the
        // tether segment is a different network from the wireless one.
        assert!(is_the_same_device(
            &peer(),
            ip("10.114.89.244"),
            from("10.114.89.244:4011"),
            &announcement("Pixel 7a", 4010),
        ));
    }

    #[test]
    fn a_different_phone_that_somehow_verified_is_still_refused() {
        // Belt and braces over the tag. The name is inside the HMAC, so this
        // can only happen if two devices were paired with one code, and the
        // cost of checking is one string comparison.
        assert!(!is_the_same_device(
            &peer(),
            ip("10.114.89.244"),
            from("10.114.89.244:4011"),
            &announcement("Galaxy S23", 4010),
        ));
    }

    #[test]
    fn an_answer_from_somewhere_other_than_where_we_asked_is_refused() {
        // A third device on the tether segment answering a probe addressed to
        // the gateway. Sending audio there is exactly the failure this whole
        // module exists to prevent.
        assert!(!is_the_same_device(
            &peer(),
            ip("10.114.89.244"),
            from("10.114.89.99:4011"),
            &announcement("Pixel 7a", 4010),
        ));
    }

    #[test]
    fn a_peer_that_moved_its_listener_is_not_migrated_onto_silently() {
        assert!(!is_the_same_device(
            &peer(),
            ip("10.114.89.244"),
            from("10.114.89.244:4011"),
            &announcement("Pixel 7a", 4999),
        ));
    }

    #[test]
    fn the_audio_port_comes_from_the_address_the_session_is_already_using() {
        assert_eq!(peer().audio_port(), 4010);
    }

    #[test]
    fn a_verification_over_an_interface_that_does_not_exist_fails_rather_than_hangs() {
        // Binding an address this machine does not hold is the ordinary
        // outcome when a cable is pulled between the poll and the probe.
        let route = Route {
            target: from("10.114.89.244:4010"),
            bind: from("10.114.89.252:0"),
            kind: LinkKind::Wired,
        };
        let started = Instant::now();
        let found = verify(&route, &peer(), &[0x5A; NONCE_BYTES]);

        assert!(found.is_none());
        assert!(
            started.elapsed() < REPLY_WINDOW * 4,
            "a dead interface must not hold the watcher up"
        );
    }

    #[test]
    fn a_stand_in_phone_on_loopback_is_verified_over_the_route_it_answers_on() {
        // The verification path end to end, minus the phone: a thread that
        // knows the code answers probes the way the receiver's announce loop
        // does, and the check has to accept it. Loopback, so it needs no
        // network and touches nothing on this machine's real interfaces.
        let peer = Peer {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 4010)),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
        };

        let responder = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        let port = responder.local_addr().unwrap().port();
        responder
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        let name = peer.name.clone();
        let code = peer.code.clone();
        let phone = std::thread::spawn(move || {
            let mut datagram = [0_u8; 256];
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let Ok((length, from)) = responder.recv_from(&mut datagram) else {
                    continue;
                };
                let Some(nonce) = discovery::probe_nonce(&datagram[..length]) else {
                    continue;
                };
                let reply = discovery::encode_announce(&name, 4010, &nonce, &code);
                let _ = responder.send_to(&reply, from);
                return;
            }
        });

        // The discovery port is fixed in the protocol, so the stand-in cannot
        // sit on it without colliding with a real receiver. Verified against a
        // route whose target names the responder's port instead, which
        // exercises everything except the constant.
        let route = Route {
            target: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            kind: LinkKind::Wireless,
        };
        let found = verify_at(&route, &peer, &[0x11; NONCE_BYTES], port);
        let _ = phone.join();

        assert!(
            found.is_some(),
            "a device holding the pairing code was not recognised"
        );
    }

    #[test]
    fn a_device_that_does_not_hold_the_pairing_code_is_not_migrated_onto() {
        // The stranger. It answers, promptly and well-formed, and it must not
        // be enough: the audio would follow it.
        let peer = Peer {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 4010)),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
        };

        let responder = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        let port = responder.local_addr().unwrap().port();
        responder
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        let stranger = PairingCode::parse("000001").unwrap();
        let phone = std::thread::spawn(move || {
            let mut datagram = [0_u8; 256];
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                let Ok((length, from)) = responder.recv_from(&mut datagram) else {
                    continue;
                };
                let Some(nonce) = discovery::probe_nonce(&datagram[..length]) else {
                    continue;
                };
                let reply = discovery::encode_announce("Pixel 7a", 4010, &nonce, &stranger);
                let _ = responder.send_to(&reply, from);
            }
        });

        let route = Route {
            target: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            kind: LinkKind::Wireless,
        };
        let found = verify_at(&route, &peer, &[0x22; NONCE_BYTES], port);
        let _ = phone.join();

        assert!(found.is_none(), "audio would have followed a stranger");
    }
}
