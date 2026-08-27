//! Does the sender actually learn from a real receiver?
//!
//! Runs the shipping send loop against a stand-in receiver that speaks the
//! real feedback protocol, then reads the telemetry the UI would have shown.
//! The point is the contrast: the same loop with nobody listening must report
//! nothing at all.
//!
//! `cargo run -p sonduit-desktop --example heartbeat_check`

#[cfg(windows)]
fn main() {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_core::packet::SonduitPacket;
    use sonduit_desktop::bridge::{capture_to_socket, BridgeSnapshot};
    use sonduit_transport::feedback::{Feedback, FEEDBACK_BYTES, FEEDBACK_INTERVAL_MS};
    use sonduit_transport::Wire;

    const RUN: Duration = Duration::from_secs(4);

    /// A receiver that answers on the protocol's own cadence.
    fn stand_in(socket: UdpSocket, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<u64> {
        std::thread::spawn(move || {
            let mut datagram = [0_u8; 2048];
            let mut out = [0_u8; FEEDBACK_BYTES];
            let mut last_report = Instant::now();
            let mut accepted = 0_u64;

            while !stop.load(Ordering::Relaxed) {
                let Ok((length, from)) = socket.recv_from(&mut datagram) else {
                    continue;
                };
                let Ok(packet) = SonduitPacket::decode(&datagram[..length]) else {
                    continue;
                };
                accepted += 1;

                if last_report.elapsed() >= Duration::from_millis(FEEDBACK_INTERVAL_MS) {
                    last_report = Instant::now();
                    let report = Feedback {
                        echo: packet.timestamp_frames,
                        hold_ms: 0,
                        accepted,
                        lost: 0,
                        depth_tenths_ms: 284,
                        playing: true,
                    };
                    if report.encode(&mut out).is_ok() {
                        let _ = socket.send_to(&out, from);
                    }
                }
            }
            accepted
        })
    }

    fn run(target: SocketAddr, receiver: Option<UdpSocket>) -> BridgeSnapshot {
        let mut capture = open(CaptureMode::EndpointLoopback, 10).expect("capture should open");
        let format = capture.format();

        let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        sender.set_nonblocking(true).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(BridgeSnapshot::default()));
        let stopper = capture.stopper();

        let listener = receiver.map(|socket| {
            socket
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            stand_in(socket, Arc::clone(&stop))
        });

        // Sampled while the session is live. The loop clears the snapshot on
        // its way out, so reading it afterwards would only ever show the
        // stopped state, which is not what the UI displays.
        let sampled = Arc::new(Mutex::new(BridgeSnapshot::default()));
        {
            let stop = Arc::clone(&stop);
            let snapshot = Arc::clone(&snapshot);
            let sampled = Arc::clone(&sampled);
            std::thread::spawn(move || {
                std::thread::sleep(RUN);
                if let (Ok(live), Ok(mut out)) = (snapshot.lock(), sampled.lock()) {
                    *out = live.clone();
                }
                stop.store(true, Ordering::Relaxed);
                stopper.stop();
            });
        }

        capture_to_socket(
            &mut capture,
            &sender,
            target,
            format,
            Wire::Sonduit,
            &stop,
            &snapshot,
        );

        if let Some(listener) = listener {
            println!("  receiver accepted {} packets", listener.join().unwrap());
        }

        let result = sampled.lock().unwrap().clone();
        result
    }

    println!("with a receiver answering:");
    let receiver = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let target = receiver.local_addr().unwrap();
    let answered = run(target, Some(receiver));
    println!("  status  {}", answered.status);
    println!("  latency {:?} ms", answered.telemetry.latency_ms);
    println!("  rtt     {:?} ms", answered.telemetry.round_trip_ms);
    println!("  depth   {:?} ms", answered.telemetry.buffer_depth_ms);
    println!("  loss    {:?} %", answered.telemetry.packet_loss_pct);
    println!(
        "  got     {:?} packets",
        answered.telemetry.packets_received
    );

    println!("with nobody listening:");
    let dead = {
        let probe = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        probe.local_addr().unwrap()
    };
    let silent = run(dead, None);
    println!("  status {}", silent.status);
    println!("  latency {:?}", silent.telemetry.latency_ms);
    println!("  loss    {:?}", silent.telemetry.packet_loss_pct);

    if silent.telemetry.latency_ms.is_some() || silent.telemetry.packet_loss_pct.is_some() {
        println!("RESULT: a figure was invented with no receiver");
    } else if answered.status != "connected" || answered.telemetry.latency_ms.is_none() {
        println!("RESULT: a receiver answered and the sender did not notice");
    } else {
        println!("RESULT: ok");
    }
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
