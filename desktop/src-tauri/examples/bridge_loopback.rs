//! Drive the whole desktop send path without a window.
//!
//! Opens real WASAPI capture, packetizes, sends over a real UDP socket to a
//! receiver on this machine, decodes what arrives and writes it to a WAV file.
//! That is every stage the phone will see except the phone itself, so a fault
//! anywhere in the glue shows up here rather than as silence on a device.
//!
//! Run with `cargo run -p sonduit-desktop --example bridge_loopback`.

#[cfg(windows)]
fn main() {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_core::packet::SonduitPacket;
    use sonduit_desktop::bridge::{capture_to_socket, BridgeSnapshot};
    use sonduit_transport::sink::{AudioSink, WavFileSink};
    use sonduit_transport::Wire;

    const RUN: Duration = Duration::from_secs(3);

    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let target = receiver.local_addr().unwrap();
    println!("receiver listening on {target}");

    let mut capture = match open(CaptureMode::EndpointLoopback, 10, None) {
        Ok(capture) => capture,
        Err(error) => {
            println!("capture failed: {error}");
            return;
        }
    };
    let format = capture.format();
    println!(
        "capturing {} Hz, {} ch, {} bit",
        format.sample_rate,
        format.channels,
        format.bit_depth.bits()
    );

    let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(BridgeSnapshot::default()));
    let stopper = capture.stopper();

    // The capture client stays on this thread: WASAPI interfaces are
    // apartment-bound, so it is the receiver that moves, not the sender.
    let path = std::env::temp_dir().join("sonduit-bridge-loopback.wav");
    let listener = {
        let path = path.clone();
        std::thread::spawn(move || {
            let mut sink = WavFileSink::create(&path, format).unwrap();
            let mut datagram = [0_u8; 1500];
            let mut received = 0_u64;
            let mut malformed = 0_u64;
            let mut frames = 0_u64;
            let mut peak = 0_i16;
            let mut first_sequence = None;
            let mut last_sequence = 0_u16;
            let deadline = Instant::now() + RUN;

            while Instant::now() < deadline {
                let Ok((length, _)) = receiver.recv_from(&mut datagram) else {
                    continue;
                };
                match SonduitPacket::decode(&datagram[..length]) {
                    Ok(packet) => {
                        received += 1;
                        first_sequence.get_or_insert(packet.sequence);
                        last_sequence = packet.sequence;
                        frames += (packet.pcm.len() / format.bytes_per_frame()) as u64;
                        for pair in packet.pcm.chunks_exact(2) {
                            peak =
                                peak.max(i16::from_le_bytes([pair[0], pair[1]]).saturating_abs());
                        }
                        sink.write(packet.pcm).unwrap();
                    }
                    Err(_) => malformed += 1,
                }
            }

            sink.finish().unwrap();
            let expected = last_sequence.wrapping_sub(first_sequence.unwrap_or(0)) as u64 + 1;
            (received, malformed, frames, peak, expected)
        })
    };

    let stopper_thread = {
        let stop = Arc::clone(&stop);
        let stopper = stopper.clone();
        std::thread::spawn(move || {
            std::thread::sleep(RUN + Duration::from_millis(200));
            stop.store(true, Ordering::Relaxed);
            stopper.stop();
        })
    };

    capture_to_socket(
        &mut capture,
        &sender,
        target,
        format,
        Wire::Sonduit,
        &stop,
        &snapshot,
    );

    let (received, malformed, frames, peak, expected) = listener.join().unwrap();
    let _ = stopper_thread.join();

    let seconds = frames as f64 / f64::from(format.sample_rate);

    println!("packets:   {received} received, {malformed} malformed");
    println!("sequences: {expected} expected across the run");
    println!(
        "frames:    {frames} ({seconds:.2}s of audio in {:.0}s wall)",
        RUN.as_secs_f64()
    );
    println!("peak:      {peak}");
    println!("wav:       {}", path.display());

    if received == 0 {
        println!("RESULT: nothing arrived. The send path is broken.");
    } else if malformed > 0 {
        println!("RESULT: malformed datagrams. Framing is wrong.");
    } else if received < expected {
        println!(
            "RESULT: {} of {expected} datagrams lost.",
            expected - received
        );
    } else if seconds < RUN.as_secs_f64() * 0.9 {
        println!("RESULT: gaps. Less audio arrived than was captured.");
    } else {
        println!("RESULT: ok");
    }
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
