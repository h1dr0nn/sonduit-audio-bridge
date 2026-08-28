//! What encryption costs one packet, in microseconds.
//!
//! ```text
//! cargo run --release --example seal_cost -p sonduit-transport
//! ```
//!
//! Run it in release. A debug build measures the borrow checker's scaffolding
//! rather than the cipher, and this number exists to be compared against the
//! 6 ms packetisation floor from `docs/protocol.md`, not against itself.
//!
//! Both halves are measured separately because they run on different threads
//! on different machines: sealing is on the desktop's capture thread and
//! opening is on the phone's receive thread, and neither has the other's
//! budget to spend.
//!
//! The sealing loop reuses one datagram buffer and the opening loop reuses one
//! plaintext buffer, which is the whole point: nothing on either path
//! allocates per packet. The batch of sealed datagrams the opening loop reads
//! is built before the clock starts.

use std::time::Instant;

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::packet::SonduitPacket;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::sealed::{Opener, Sealer};
use sonduit_transport::session::{KeyExchange, SessionSecret, SALT_BYTES, SEED_BYTES};

/// Packets sealed per batch, and so packets held in memory at once.
const BATCH: usize = 4096;

/// Batches. `BATCH * BATCHES` is the sample count for each figure.
const BATCHES: usize = 16;

/// One packet of the baseline format, from `docs/protocol.md`.
const PACKET_MICROS: f64 = 6000.0;

fn paired() -> (SessionSecret, SessionSecret) {
    let nonce = [0x5A_u8; NONCE_BYTES];
    let code = PairingCode::parse("482913").expect("a six digit code");
    let desktop = KeyExchange::from_seed([11; SEED_BYTES]);
    let phone = KeyExchange::from_seed([22; SEED_BYTES]);
    let (pa, pb) = (desktop.public_key(), phone.public_key());

    let sending = desktop
        .agree(&pb, &nonce, &code, &pa, &pb)
        .expect("a contributory exchange");
    let receiving = phone
        .agree(&pa, &nonce, &code, &pa, &pb)
        .expect("a contributory exchange");
    (sending, receiving)
}

fn report(what: &str, total_nanos: u128, packets: usize) -> f64 {
    let micros = total_nanos as f64 / 1000.0 / packets as f64;
    println!(
        "{what:<28} {micros:>8.3} us/packet   {:>6.3}% of the 6 ms budget",
        micros / PACKET_MICROS * 100.0
    );
    micros
}

fn main() {
    let format = Format::stereo_48k();
    let (sending, receiving) = paired();
    let pcm: Vec<u8> = (0..PCM_PAYLOAD_BYTES)
        .map(|index| (index % 251) as u8)
        .collect();

    println!(
        "payload {PCM_PAYLOAD_BYTES} bytes, sealed datagram {} bytes, {BATCH} x {BATCHES} packets",
        Sealer::sealed_len(PCM_PAYLOAD_BYTES)
    );
    println!();

    // --- the cleartext encode, for scale ----------------------------------
    let mut plain_datagram = vec![0_u8; SonduitPacket::encoded_len(PCM_PAYLOAD_BYTES)];
    let mut plain_nanos = 0_u128;
    for _ in 0..BATCHES {
        let started = Instant::now();
        for sequence in 0..BATCH {
            SonduitPacket {
                format,
                sequence: sequence as u16,
                timestamp_frames: sequence as u32,
                flags: 0,
                pcm: &pcm,
            }
            .encode(&mut plain_datagram)
            .expect("encode");
        }
        plain_nanos += started.elapsed().as_nanos();
    }
    let plain = report("version 1 encode", plain_nanos, BATCH * BATCHES);

    // --- sealing ----------------------------------------------------------
    let mut datagram = vec![0_u8; Sealer::sealed_len(PCM_PAYLOAD_BYTES)];
    let mut seal_nanos = 0_u128;
    for batch in 0..BATCHES {
        let mut sealer = Sealer::new(&sending, [batch as u8; SALT_BYTES]);
        let started = Instant::now();
        for _ in 0..BATCH {
            sealer
                .seal(&format, 0, 0, &pcm, &mut datagram)
                .expect("seal");
        }
        seal_nanos += started.elapsed().as_nanos();
    }
    let seal = report("seal", seal_nanos, BATCH * BATCHES);

    // --- opening ----------------------------------------------------------
    //
    // A fresh Opener per batch, because the replay window refuses a packet it
    // has already accepted and re-sending one would be measuring the window
    // rather than the cipher.
    let mut plaintext = vec![0_u8; PCM_PAYLOAD_BYTES];
    let mut open_nanos = 0_u128;
    for batch in 0..BATCHES {
        let mut sealer = Sealer::new(&sending, [batch as u8; SALT_BYTES]);
        let mut sealed = Vec::with_capacity(BATCH);
        for _ in 0..BATCH {
            let mut one = vec![0_u8; Sealer::sealed_len(PCM_PAYLOAD_BYTES)];
            sealer.seal(&format, 0, 0, &pcm, &mut one).expect("seal");
            sealed.push(one);
        }

        let mut opener = Opener::new(receiving.clone());
        let started = Instant::now();
        for one in &sealed {
            opener.open(one, &mut plaintext).expect("open");
        }
        open_nanos += started.elapsed().as_nanos();
    }
    let open = report("open", open_nanos, BATCH * BATCHES);

    println!();
    println!(
        "encryption adds {:.3} us on the sender and {:.3} us on the receiver",
        seal - plain,
        open
    );
    println!(
        "worst of the two is {:.3}% of the 6 ms a packet lasts",
        seal.max(open) / PACKET_MICROS * 100.0
    );
}
