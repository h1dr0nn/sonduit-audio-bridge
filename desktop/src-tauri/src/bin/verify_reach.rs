//! Ask a real phone where it can be reached, over every route this machine
//! has, and show which of those questions get an answer.
//!
//! This is the driver for the discovery half of the bridge: `bridge::discover`
//! and `peer::find_elsewhere` both hinge on a limited broadcast, and a limited
//! broadcast does not cross a subnet boundary. On a network where the phone's
//! Wi-Fi lease is on a different subnet from the desktop, that is the
//! difference between a session that can retreat off a failing cable and one
//! that is armed onto nothing.
//!
//! It runs three things against the device that is actually plugged in:
//!
//! 1. `bridge::discover`, which is what the typed-code path does.
//! 2. `peer::find_elsewhere` with nothing remembered, which is what a session
//!    paired over the cable used to have.
//! 3. `peer::find_elsewhere` with the phone's wireless address remembered,
//!    which is what a QR pairing now leaves behind.
//!
//! The address for step 3 is supplied on the command line because a headless
//! driver cannot hold a QR code up to a camera. It is the same address the
//! pairing would have collected on its own: the phone announces to every
//! address in the invite, so a copy arrives over each link carrying that
//! link's source address. `verify_session` documents the same substitution for
//! the same reason.
//!
//! `cargo run -p sonduit-desktop --bin verify_reach -- <code> [phone-wifi-ip]`
//!
//! It lives under `src/bin` rather than `examples/` because `tauri-build`
//! emits its link arguments with `cargo:rustc-link-arg-bins`, which does not
//! reach an example target: an example that builds a Tauri app links but will
//! not load, exiting 0xC0000139 before `main` runs.

#[cfg(windows)]
fn main() {
    use std::net::SocketAddr;
    use std::time::Instant;

    use sonduit_desktop::bridge::{self, adapters, link, peer, BridgeState, LinkKind};
    use sonduit_transport::pairing::PairingCode;

    let mut arguments = std::env::args().skip(1);
    let code = arguments.next().unwrap_or_default();
    let wireless = arguments.next();

    let Some(credential) = PairingCode::parse(&code) else {
        println!("usage: verify_reach <six-digit-code> [phone-wifi-ip]");
        return;
    };

    let started = Instant::now();
    let stamp = || format!("{:7.3}s", started.elapsed().as_secs_f64());

    for adapter in adapters::enumerate().unwrap_or_default() {
        println!(
            "[{}] adapter {:?} local={} gateway={}",
            stamp(),
            adapter.description,
            adapter.local,
            adapter.gateway
        );
    }

    // 1. The typed-code path, exactly as the button runs it.
    println!("[{}] discover({code})", stamp());
    let found = match bridge::discover(&code) {
        Ok(found) => found,
        Err(error) => {
            println!("[{}] scan failed: {error}", stamp());
            return;
        }
    };
    if found.is_empty() {
        println!("[{}] nothing answered the probe", stamp());
        return;
    }
    for paired in &found {
        println!(
            "[{}] found {} at {}",
            stamp(),
            paired.device.name,
            paired.device.id
        );
    }

    let state = BridgeState::default();
    state.remember(&found, &credential);
    let primary: SocketAddr = found[0]
        .device
        .id
        .parse()
        .expect("discover reports an address");
    let peer = state.peer_at(primary).expect("just remembered");
    println!("[{}] peer knows {:?}", stamp(), peer.addresses());

    // The classification the link watcher passes in, built from the same
    // adapter list it would have walked.
    let adapters = adapters::enumerate().unwrap_or_default();
    let classify = |from: SocketAddr| {
        if link::is_tether_gateway(from.ip(), &adapters) {
            LinkKind::Wired
        } else {
            LinkKind::Wireless
        }
    };

    // 2. What a session paired over the cable had to work with.
    let at = Instant::now();
    let blind = peer::find_elsewhere(&peer, &[0x11; 16], classify);
    println!(
        "[{}] find_elsewhere with nothing remembered -> {:?} ({:.0} ms)",
        stamp(),
        blind.as_ref().map(|route| route.target),
        at.elapsed().as_secs_f64() * 1000.0
    );

    // 3. The same call, with the address a QR pairing would have collected.
    let Some(wireless) = wireless else {
        println!("[{}] no wireless address given; stopping here", stamp());
        return;
    };
    let Ok(hint) = format!("{wireless}:{}", peer.audio_port()).parse::<SocketAddr>() else {
        println!("[{}] {wireless} is not an address", stamp());
        return;
    };

    let mut peer = peer;
    peer.elsewhere = vec![hint];
    println!("[{}] peer knows {:?}", stamp(), peer.addresses());

    let at = Instant::now();
    let hinted = peer::find_elsewhere(&peer, &[0x22; 16], classify);
    println!(
        "[{}] find_elsewhere with {hint} remembered -> {:?} ({:.0} ms)",
        stamp(),
        hinted.as_ref().map(|route| route.target),
        at.elapsed().as_secs_f64() * 1000.0
    );

    // And the safety property, against the same live phone: the address is
    // right and something does answer there, but under a code this desktop
    // never agreed. Being remembered must not get it past the check.
    let Some(stranger) = PairingCode::parse("000001") else {
        return;
    };
    let mut impostor = peer.clone();
    impostor.code = stranger;
    let refused = peer::find_elsewhere(&impostor, &[0x33; 16], classify);
    println!(
        "[{}] find_elsewhere at the same address under the wrong code -> {:?}",
        stamp(),
        refused.as_ref().map(|route| route.target)
    );
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
