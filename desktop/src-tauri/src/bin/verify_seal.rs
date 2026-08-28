//! Agree a key with the phone by hand, seal audio under it, and print the wire.
//!
//! Two jobs, both of which the application cannot do for itself:
//!
//! * it prints the exact bytes handed to `sendto`, so what crosses the
//!   interface can be looked at rather than taken on the strength of a flag;
//! * it takes the number of key offers as an argument, which is the one
//!   variable that separates a session the phone plays from a session the
//!   phone refuses.
//!
//! Nothing here reimplements the cipher: `Offer`, `Sealer` and `Packetizer`
//! are the same public types the desktop uses.
//!
//! `cargo run -p sonduit-desktop --bin verify_seal -- <code> <phone-ip> <offers> <packets>`

fn main() {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use sonduit_core::format::{BitDepth, Format};
    use sonduit_transport::handshake::Offer;
    use sonduit_transport::packetize::Packetizer;
    use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
    use sonduit_transport::sealed::Sealer;
    use sonduit_transport::{discovery, entropy};

    let mut arguments = std::env::args().skip(1);
    let code = arguments.next().unwrap_or_default();
    let host = arguments.next().unwrap_or_default();
    let offers: u32 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let packets: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(200);

    let Some(credential) = PairingCode::parse(&code) else {
        println!("usage: verify_seal <six-digit-code> <phone-ip> [offers] [packets]");
        return;
    };
    let Ok(ip) = host.parse::<Ipv4Addr>() else {
        println!("that is not an IPv4 address");
        return;
    };

    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64())
    };

    let mut nonce = [0_u8; NONCE_BYTES];
    entropy::fill(&mut nonce).expect("the system random source must answer");

    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let discovery_at = SocketAddr::new(ip.into(), discovery::DISCOVERY_PORT);

    // Probe, exactly as a scan does, so the responder has a nonce on record
    // for the offer that follows to be answered against.
    let probe = discovery::encode_probe(&nonce);
    for _ in 0..3 {
        let _ = socket.send_to(&probe, discovery_at);
    }

    let mut datagram = [0_u8; 256];
    let deadline = Instant::now() + Duration::from_millis(1_500);
    let mut announced = None;
    while Instant::now() < deadline && announced.is_none() {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        if let Some(announcement) =
            discovery::decode_announce(&datagram[..length], &nonce, &credential)
        {
            announced = Some(discovery::audio_address(from, &announcement));
        }
    }
    let Some(audio_at) = announced else {
        println!("nothing answered the probe at {discovery_at}");
        return;
    };
    println!("[{:.4}] receiver announced audio on {audio_at}", now());

    // The one variable under test. The desktop sends three of these and keeps
    // the first accept that comes back; the receiver answers every one of them
    // with a fresh key pair and keeps the last.
    let seed = entropy::key_seed().expect("the system random source must answer");
    let offer = Offer::new(seed, nonce, credential.clone());
    let offer_datagram = offer.datagram();
    for _ in 0..offers {
        let _ = socket.send_to(&offer_datagram, discovery_at);
    }
    println!("[{:.4}] sent {offers} key offer(s)", now());

    let deadline = Instant::now() + Duration::from_millis(600);
    let mut accepts: Vec<Vec<u8>> = Vec::new();
    while Instant::now() < deadline {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        if from.ip() != discovery_at.ip() {
            continue;
        }
        if offer.is_our_accept(&datagram[..length]) {
            accepts.push(datagram[..length].to_vec());
        }
    }
    println!("[{:.4}] {} accept(s) came back", now(), accepts.len());
    if accepts.is_empty() {
        println!("no accept: nothing to key from");
        return;
    }
    // The first one, which is what `bridge::agree_key` keeps.
    let Some(secret) = offer.accept(&accepts[0]) else {
        println!("the first accept did not verify");
        return;
    };

    let format = Format {
        sample_rate: 48_000,
        bit_depth: BitDepth::S16,
        channels: 2,
        channel_mask: 0x0003,
    };
    let salt = entropy::stream_salt().expect("the system random source must answer");
    let sealer = Sealer::new(&secret, salt);
    let mut packetizer = Packetizer::sealed(format, sealer).on_wired_link(false);

    // A real tone, so what goes in is known and what comes out can be compared
    // against it.
    let frames = 288_usize; // 1152 bytes, one packet's worth
    let mut phase = 0_u64;
    let mut shown = false;
    let mut sent = 0_u64;
    let mut failures = 0_u64;

    for _ in 0..packets {
        let mut pcm = Vec::with_capacity(frames * 4);
        for _ in 0..frames {
            let value = (8000.0
                * (2.0 * std::f64::consts::PI * 440.0 * phase as f64 / 48_000.0).sin())
                as i16;
            pcm.extend_from_slice(&value.to_le_bytes());
            pcm.extend_from_slice(&value.to_le_bytes());
            phase += 1;
        }
        if !shown {
            println!("cleartext PCM about to be sealed, first 24 bytes:");
            println!("  {}", hex(&pcm[..24]));
        }
        let result = packetizer.push(&pcm, |wire| {
            if !shown {
                shown = true;
                println!("what leaves the socket, {} bytes:", wire.len());
                println!("  first 44: {}", hex(&wire[..44.min(wire.len())]));
                println!("  version byte: {}", wire[4]);
                let plain = &pcm[..];
                let contains = wire
                    .windows(plain.len().min(64))
                    .any(|window| window == &plain[..plain.len().min(64)]);
                println!("  contains the first 64 PCM bytes verbatim: {contains}");
                println!(
                    "  distinct byte values in payload: {}",
                    distinct(&wire[32..])
                );
            }
            match socket.send_to(wire, audio_at) {
                Ok(_) => Ok(()),
                Err(error) => Err(sonduit_transport::TransportError::Io(error)),
            }
        });
        match result {
            Ok(()) => sent += 1,
            Err(_) => failures += 1,
        }
        std::thread::sleep(Duration::from_millis(6));
    }

    println!(
        "[{:.4}] pushed {sent} blocks ({} packets), {failures} failures",
        now(),
        packetizer.packets()
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How many of the 256 byte values appear. A weak signal on its own -- a
/// 1152-byte tone already uses 189 of them -- and reported only alongside the
/// version byte and the verbatim-PCM search, which are the two that decide.
fn distinct(bytes: &[u8]) -> usize {
    let mut seen = [false; 256];
    for byte in bytes {
        seen[*byte as usize] = true;
    }
    seen.iter().filter(|present| **present).count()
}
