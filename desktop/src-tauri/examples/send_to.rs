//! Send captured system audio to a real device.
//!
//! The same path the application uses, addressable from the command line so a
//! phone can be tested without clicking through the window.
//!
//! `cargo run -p sonduit-desktop --example send_to -- <ip:port> [seconds]`

#[cfg(windows)]
fn main() {
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_desktop::bridge::{capture_to_socket, BridgeSnapshot};
    use sonduit_transport::Wire;

    let mut arguments = std::env::args().skip(1);
    let Some(target) = arguments
        .next()
        .and_then(|text| text.parse::<SocketAddr>().ok())
    else {
        println!("usage: send_to <ip:port> [seconds]");
        return;
    };
    let seconds: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    let mut capture = match open(CaptureMode::EndpointLoopback, 10) {
        Ok(capture) => capture,
        Err(error) => {
            println!("capture failed: {error}");
            return;
        }
    };
    let format = capture.format();
    println!(
        "capturing {} Hz, {} ch",
        format.sample_rate, format.channels
    );

    // Bound to the interface that reaches the target, so the routing table
    // cannot send this out of the wrong adapter.
    let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).unwrap();
    socket.connect(target).unwrap();
    let local = socket.local_addr().unwrap();
    println!("sending {local} -> {target} for {seconds}s");

    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(BridgeSnapshot::default()));
    let stopper = capture.stopper();

    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            stop.store(true, Ordering::Relaxed);
            stopper.stop();
        });
    }

    capture_to_socket(
        &mut capture,
        &socket,
        target,
        format,
        Wire::Sonduit,
        &stop,
        &snapshot,
    );

    let final_snapshot = snapshot.lock().unwrap().clone();
    println!("sent {:?} packets", final_snapshot.telemetry.packets_sent);
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
