//! Stream to a phone over Wi-Fi and record what the far end reports, for a
//! before-and-after measurement of the jitter buffer's depth.
//!
//! `bridge::start` cannot be used here for the same reason `verify_follow`
//! cannot: it seals every Sonduit session from a `PairedDevice` this driver
//! has no way to build, because `discover` reaches a phone only by limited
//! broadcast or a tether gateway and this network has the phone's Wi-Fi lease
//! on another subnet. So this drives the same `capture_and_follow` loop with
//! `keying: None`, the version 1 cleartext wire an unpaired receiver accepts.
//! Everything downstream of the wire -- the jitter buffer, the hand-off ring,
//! the drift controller -- is the product's, unchanged.
//!
//! No link offers and no standby: this is one route for the whole run, so the
//! only thing moving the receiver's depth is the access point.
//!
//! `cargo run -p sonduit-desktop --features verify --bin verify_wifi_depth -- <phone-ip> <seconds>`

#[cfg(windows)]
fn main() {
    use std::net::Ipv4Addr;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_desktop::bridge::{capture_and_follow, BridgeSnapshot, Link, LinkKind, Route};
    use sonduit_transport::{Wire, DEFAULT_PORT};

    let mut arguments = std::env::args().skip(1);
    let wifi: Ipv4Addr = arguments
        .next()
        .and_then(|text| text.parse().ok())
        .expect("a phone address on the wireless network");
    let seconds: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(300);

    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64())
    };

    let home = Route::unbound(
        SocketAddr::new(wifi.into(), DEFAULT_PORT),
        LinkKind::Wireless,
    );
    let opening = Link::bind(home.clone()).expect("the wireless route must bind");
    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(BridgeSnapshot::default()));

    let mut capture = open(CaptureMode::EndpointLoopback, 10, None).expect("capture must open");
    let format = capture.format();
    let stopper = capture.stopper();
    println!(
        "[{:.4}] capturing {} Hz {} ch, sending cleartext sonduit to {}",
        now(),
        format.sample_rate,
        format.channels,
        home.target
    );
    println!("epoch\telapsed\tsent\trecv\tloss_pct\trtt_ms\tdepth_ms\tlatency_ms\tjitter_ms\tlate\treordered\taudio_s\terr");

    // A quarter of a second, which is one feedback report: sampling faster
    // republishes the same report and would weight the distribution by how
    // often this loop ran rather than by how often the phone spoke.
    {
        let stop = Arc::clone(&stop);
        let snapshot = Arc::clone(&snapshot);
        let started = Instant::now();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                let view = match snapshot.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => continue,
                };
                println!(
                    "[{:.4}]\t{:.3}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}\t{:?}",
                    now(),
                    started.elapsed().as_secs_f64(),
                    view.telemetry.packets_sent,
                    view.telemetry.packets_received,
                    view.telemetry.packet_loss_pct,
                    view.telemetry.round_trip_ms,
                    view.telemetry.buffer_depth_ms,
                    view.telemetry.latency_ms,
                    view.telemetry.jitter_ms,
                    view.telemetry.late_packets,
                    view.telemetry.reordered_packets,
                    view.telemetry.audio_seconds,
                    view.error
                );
            }
        });
    }

    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            stop.store(true, Ordering::Relaxed);
            stopper.stop();
        });
    }

    let started = Instant::now();
    capture_and_follow(
        &mut capture,
        opening,
        format,
        Wire::Sonduit,
        &stop,
        &snapshot,
        None,
        None,
    );
    let final_snapshot = snapshot.lock().expect("not poisoned").clone();
    println!(
        "[{:.4}] finished after {:.1}s: sent={:?} recv={:?} loss={:?} late={:?} err={:?}",
        now(),
        started.elapsed().as_secs_f64(),
        final_snapshot.telemetry.packets_sent,
        final_snapshot.telemetry.packets_received,
        final_snapshot.telemetry.packet_loss_pct,
        final_snapshot.telemetry.late_packets,
        final_snapshot.error
    );
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
