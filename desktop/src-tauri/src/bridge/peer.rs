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
use sonduit_transport::session::SessionSecret;

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
///
/// Both halves of one pairing live here because both come out of it and
/// neither outlives it: the code proves, later and over another path, that a
/// phone answering on a cable is this phone, and the secret is what the audio
/// to that phone is keyed from. Keeping them in two lists indexed by address
/// is two lists that can disagree about which pairing is current.
///
/// `Debug` is derived and safe to print: [`PairingCode`] and [`SessionSecret`]
/// each redact themselves, and logs get copied into bug reports.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Where it answered from when it was paired or scanned.
    pub address: SocketAddr,
    /// Other addresses this same device answered from during that pairing.
    ///
    /// A phone that is tethered and on Wi-Fi at once holds an address on both
    /// networks, and the QR pairing path already puts both in front of this
    /// desktop: the invite offers every local address, the phone unicasts its
    /// announcement to all of them, and one copy arrives over each link with
    /// that link's source address on it. Only the first was ever kept.
    ///
    /// **These are hints about where to look, and nothing more.** No address
    /// in here is a reason to send audio anywhere or to believe anything that
    /// answers there. Every one of them is probed exactly as a broadcast
    /// candidate is, and accepted only if what answers produces the pairing
    /// tag under [`Self::code`] and announces the same name and audio port --
    /// which is the check [`is_the_same_device`] and
    /// [`sonduit_transport::discovery::decode_announce`] were already doing.
    /// A stale address costs one unanswered datagram.
    pub elsewhere: Vec<SocketAddr>,
    /// The name it announced. Covered by the tag, so it cannot be rewritten.
    pub name: String,
    /// The pairing code that proved it. This is the credential every later
    /// check is made against.
    pub code: PairingCode,
    /// The master secret agreed with it, which every stream to it is keyed
    /// from. See ADR-009 and [`sonduit_transport::session`].
    ///
    /// Never serialised, never logged and never sent anywhere: the only thing
    /// that reads it is [`super::start`], which turns it into a
    /// [`sonduit_transport::sealed::Sealer`] on the capture thread.
    pub secret: SessionSecret,
}

impl Peer {
    /// The port the peer listens for audio on.
    #[must_use]
    pub const fn audio_port(&self) -> u16 {
        self.address.port()
    }

    /// Every address worth asking, best first, without repeats.
    ///
    /// The paired address leads because it is the one this session was set up
    /// against and the one most likely to still answer. The rest follow in
    /// the order they were learned.
    ///
    /// Deliberately includes the paired address even when the caller is
    /// looking for somewhere else to go: whether an address is "somewhere
    /// else" is decided by what link its answer classifies as, not by which
    /// list it came out of. A session that started on Wi-Fi and moved onto a
    /// cable has its wireless route sitting in exactly this field.
    #[must_use]
    pub fn addresses(&self) -> Vec<SocketAddr> {
        let mut all = Vec::with_capacity(1 + self.elsewhere.len());
        all.push(self.address);
        for address in &self.elsewhere {
            if !all.contains(address) {
                all.push(*address);
            }
        }
        all
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
/// Answers that classify as wired are rejected, since the point of asking is
/// to have somewhere to retreat to when the cable goes.
///
/// # Why this asks twice
///
/// It broadcasts, because the phone's address on the wireless network can
/// change with the DHCP lease and the address this session knows may have
/// expired. **And it asks every address the phone has ever answered from**,
/// because a limited broadcast does not cross a subnet boundary and on a
/// routed network that is the only question that gets an answer.
///
/// Measured on a network where the desktop sits on `10.10.0.0/22` and the
/// phone's Wi-Fi lease is on `10.10.20.0/22`: `255.255.255.255` leaves by
/// whichever interface the routing table prefers and is answered by nothing
/// at all, while a unicast probe to the phone's wireless address is answered
/// in four milliseconds. With only the broadcast, a session that started on
/// the cable had nowhere to retreat to, and the standby was armed onto
/// nothing -- which is the failure this function exists to prevent.
///
/// # Why remembering an address does not weaken anything
///
/// Nothing here trusts an address. A remembered address decides only where a
/// probe is sent; what may be migrated onto is still decided by the answer,
/// and the answer still has to carry an HMAC over this probe's fresh nonce,
/// keyed by the pairing code, and announce the same name and audio port. That
/// is the identical bar a broadcast reply has to clear. See [`Peer::elsewhere`].
///
/// # Cost
///
/// One socket and one reply window, whatever the number of candidates: every
/// probe goes out before anything is listened for. Three datagrams per
/// candidate, and a candidate that has gone is a datagram nobody answers.
///
/// `classify` is passed in rather than called, so the caller supplies the
/// adapter list it already walked this poll instead of walking it again, and
/// so this stays testable without one.
#[must_use]
pub fn find_elsewhere<F>(peer: &Peer, nonce: &[u8; NONCE_BYTES], classify: F) -> Option<Route>
where
    F: Fn(SocketAddr) -> LinkKind,
{
    find_elsewhere_at(peer, nonce, classify, discovery::DISCOVERY_PORT)
}

/// [`find_elsewhere`], with the discovery port named rather than assumed.
///
/// Exists for the same reason [`verify_at`] does: a stand-in receiver in a
/// test cannot bind the protocol's port without colliding with a live phone.
#[must_use]
fn find_elsewhere_at<F>(
    peer: &Peer,
    nonce: &[u8; NONCE_BYTES],
    classify: F,
    port: u16,
) -> Option<Route>
where
    F: Fn(SocketAddr) -> LinkKind,
{
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket.set_broadcast(true).ok()?;
    socket.set_read_timeout(Some(READ_SLICE)).ok()?;

    let mut sent = probe(
        &socket,
        SocketAddr::from((Ipv4Addr::BROADCAST, port)),
        nonce,
    );
    for address in peer.addresses() {
        sent |= probe(&socket, SocketAddr::new(address.ip(), port), nonce);
    }
    // False means not one probe left the machine, which is an interface that
    // has gone rather than a peer that is not there. Waiting out the reply
    // window for an answer to a question nobody was asked is the one case
    // worth short-circuiting.
    if !sent {
        return None;
    }

    // A tethered phone is on both segments and answers twice. Rejecting the
    // wired reply rather than taking the first one is what stops the fallback
    // being a second name for the link that has just failed.
    //
    // No `asked` address to hold an answer to, because a broadcast was not
    // addressed to anybody; the pairing tag is what stands in for it, and it
    // is the check that was doing the work in the first place. The unicast
    // probes are held to the same rule rather than to their own: a reply is
    // taken on the strength of its tag, so it does not matter which of the
    // several probes on this socket provoked it.
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

/// A master secret from a real handshake, for tests elsewhere in this crate.
///
/// The four datagrams rather than fabricated key material: a test that starts
/// from a hand-made secret proves nothing about the path a session takes, and
/// the seeds are fixed so the result is reproducible.
#[cfg(test)]
pub(crate) fn agreed_secret() -> SessionSecret {
    use sonduit_transport::handshake::{Offer, Responder};
    use sonduit_transport::session::SEED_BYTES;

    let nonce = [0x5A_u8; NONCE_BYTES];
    let code = PairingCode::parse("482913").expect("a six digit code");
    let offer = Offer::new([1; SEED_BYTES], nonce, code.clone());
    let accept = Responder::new()
        .answer(&offer.datagram(), &[nonce], &code, [2; SEED_BYTES])
        .expect("the offer verifies")
        .accept;
    offer.accept(&accept).expect("the accept verifies")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> Peer {
        Peer {
            address: "192.168.1.42:4010".parse().unwrap(),
            elsewhere: Vec::new(),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
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
            elsewhere: Vec::new(),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
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
    fn the_addresses_to_ask_lead_with_the_one_the_session_was_paired_against() {
        let mut peer = peer();
        peer.elsewhere = vec![
            from("10.10.22.160:4010"),
            // A repeat of the paired address, which the pairing window can
            // produce when the phone announces twice over the same link.
            from("192.168.1.42:4010"),
        ];

        assert_eq!(
            peer.addresses(),
            vec![from("192.168.1.42:4010"), from("10.10.22.160:4010")],
        );
    }

    /// A stand-in phone that answers probes the way the receiver's announce
    /// loop does, tagging its reply with `code`.
    ///
    /// Returns the port it is listening on. It answers until the deadline
    /// rather than once, because these tests send several probes.
    fn stand_in_phone(code: PairingCode, name: &'static str) -> (u16, std::thread::JoinHandle<()>) {
        let responder = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        let port = responder.local_addr().unwrap().port();
        responder
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();

        let handle = std::thread::spawn(move || {
            let mut datagram = [0_u8; 256];
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let Ok((length, from)) = responder.recv_from(&mut datagram) else {
                    continue;
                };
                let Some(nonce) = discovery::probe_nonce(&datagram[..length]) else {
                    continue;
                };
                let reply = discovery::encode_announce(name, 4010, &nonce, &code);
                let _ = responder.send_to(&reply, from);
            }
        });

        (port, handle)
    }

    #[test]
    fn a_peer_the_broadcast_cannot_reach_is_found_where_it_was_last_seen() {
        // The defect, in one test. The paired address is on a network this
        // machine cannot reach and the broadcast is answered by nobody, which
        // is exactly what a routed network looks like from here: the session
        // had nowhere to retreat to and the standby was armed onto nothing.
        // The address the phone answered from during the pairing is the whole
        // of the new information, and it is enough.
        let (port, phone) = stand_in_phone(PairingCode::parse("482913").unwrap(), "Pixel 7a");

        let peer = Peer {
            // TEST-NET-2, which is guaranteed to route nowhere, standing in
            // for a lease that has expired or a subnet across a router.
            address: from("198.51.100.7:4010"),
            elsewhere: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 4010))],
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
        };

        let found = find_elsewhere_at(&peer, &[0x33; NONCE_BYTES], |_| LinkKind::Wireless, port);
        let _ = phone.join();

        assert_eq!(
            found.map(|route| route.target),
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 4010))),
            "the phone answered where it was last seen and was not taken"
        );
    }

    #[test]
    fn a_remembered_address_is_where_to_look_and_never_who_to_believe() {
        // The whole safety argument for remembering an address at all. The
        // address is right, something is listening on it, and it answers
        // promptly with the correct name and port -- and it does not hold the
        // pairing code, so the audio must not follow it. Nothing about having
        // been remembered gets a device past the check.
        let (port, stranger) = stand_in_phone(PairingCode::parse("000001").unwrap(), "Pixel 7a");

        let peer = Peer {
            address: from("198.51.100.7:4010"),
            elsewhere: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 4010))],
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
        };

        let found = find_elsewhere_at(&peer, &[0x44; NONCE_BYTES], |_| LinkKind::Wireless, port);
        let _ = stranger.join();

        assert!(found.is_none(), "audio would have followed a stranger");
    }

    #[test]
    fn a_remembered_address_that_answers_over_the_cable_is_still_not_a_retreat() {
        // The point of asking is somewhere to go when the cable fails, so an
        // answer that classifies as the cable is no answer at all -- however
        // it was found. Remembering addresses must not become a way round
        // that rule.
        let (port, phone) = stand_in_phone(PairingCode::parse("482913").unwrap(), "Pixel 7a");

        let peer = Peer {
            address: from("198.51.100.7:4010"),
            elsewhere: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 4010))],
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
        };

        let found = find_elsewhere_at(&peer, &[0x55; NONCE_BYTES], |_| LinkKind::Wired, port);
        let _ = phone.join();

        assert!(found.is_none(), "the fallback is the link that just failed");
    }

    #[test]
    fn a_device_that_does_not_hold_the_pairing_code_is_not_migrated_onto() {
        // The stranger. It answers, promptly and well-formed, and it must not
        // be enough: the audio would follow it.
        let peer = Peer {
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 4010)),
            elsewhere: Vec::new(),
            name: "Pixel 7a".to_string(),
            code: PairingCode::parse("482913").unwrap(),
            secret: agreed_secret(),
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
