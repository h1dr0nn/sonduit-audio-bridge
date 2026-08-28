//! Exercise the send loop's half of link migration with audio the phone will
//! actually play.
//!
//! `bridge::start` cannot be used for this: it seals every Sonduit session, and
//! the key it agrees does not match the one the receiver keeps (see the
//! verification notes), so nothing the phone receives is ever played. This
//! drives the same `capture_and_follow` loop with `keying: None`, which is the
//! version 1 cleartext wire an unpaired receiver accepts, so the receiver side
//! of a migration -- the gap in the audio, and the jitter buffer retuning for
//! the new link -- can be observed at all.
//!
//! What this does NOT test is `watch_link` and `migrate::Policy`: the offers
//! below are made on a timer by this file rather than by the product's
//! watcher. Those are covered separately by `verify_session`.
//!
//! `cargo run -p sonduit-desktop --bin verify_follow -- <phone-wifi-ip> <seconds> <offer-at>`

#[cfg(windows)]
fn main() {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_desktop::bridge::{
        adapters, capture_and_follow, BridgeSnapshot, Link, LinkKind, LinkSwitch, Route,
    };
    use sonduit_transport::{Wire, DEFAULT_PORT};

    let mut arguments = std::env::args().skip(1);
    let wifi: Ipv4Addr = arguments
        .next()
        .and_then(|text| text.parse().ok())
        .expect("a phone address on the wireless network");
    let seconds: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let offer_at: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(10);

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
    let switch = Arc::new(LinkSwitch::new(home.clone()));
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

    // The stand-in watcher. It offers the cable once, arms the wireless
    // standby, and then leaves the send loop alone: everything after this is
    // the product's code deciding what to do.
    {
        let switch = Arc::clone(&switch);
        let stop = Arc::clone(&stop);
        let home = home.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(offer_at));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let found = adapters::enumerate().unwrap_or_default();
            let Some(cable) = found
                .iter()
                .find(|adapter| adapters::looks_like_tether(&adapter.description))
            else {
                println!("[{:.4}] no tether adapter to offer", now());
                return;
            };
            let route = Route::over(cable, DEFAULT_PORT);
            match Link::bind(route.clone()) {
                Ok(link) => {
                    println!("[{:.4}] OFFER {} over usb", now(), route.target);
                    switch.offer(link);
                }
                Err(error) => println!("[{:.4}] could not bind the cable: {error}", now()),
            }
            // The way back, bound now so taking it later is a pointer swap.
            match Link::bind(home) {
                Ok(link) => {
                    println!("[{:.4}] ARM the wireless standby", now());
                    switch.arm(link);
                }
                Err(error) => println!("[{:.4}] could not arm: {error}", now()),
            }
        });
    }

    // A prober on the cable, so the instant the interface stops accepting
    // datagrams is timed rather than inferred from an adapter list that is
    // walked far too slowly to see it.
    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let socket = match UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))) {
                Ok(socket) => socket,
                Err(_) => return,
            };
            let mut target: Option<SocketAddr> = None;
            let mut failing_since: Option<f64> = None;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(2));
                if target.is_none() {
                    target = adapters::enumerate()
                        .unwrap_or_default()
                        .iter()
                        .find(|a| adapters::looks_like_tether(&a.description))
                        .map(|a| a.target(9));
                    continue;
                }
                let to = target.expect("just set");
                match socket.send_to(b"x", to) {
                    Ok(_) => failing_since = None,
                    Err(error) => {
                        if failing_since.is_none() {
                            let at = now();
                            failing_since = Some(at);
                            println!("[{at:.4}] CABLE REFUSES DATAGRAMS: {error}");
                        }
                    }
                }
            }
        });
    }

    // Watching what the send loop publishes, fast enough to time a retreat.
    {
        let switch = Arc::clone(&switch);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut last = String::new();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(2));
                let Some(route) = switch.live() else { continue };
                let text = format!("{} via {}", route.target, route.kind.label());
                if text != last {
                    println!("[{:.4}] LIVE {text}", now());
                    last = text;
                }
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
        Some(&switch),
        None,
    );
    let final_snapshot = snapshot.lock().expect("not poisoned").clone();
    println!(
        "[{:.4}] finished after {:.1}s: sent={:?} recv={:?} loss={:?} err={:?}",
        now(),
        started.elapsed().as_secs_f64(),
        final_snapshot.telemetry.packets_sent,
        final_snapshot.telemetry.packets_received,
        final_snapshot.telemetry.packet_loss_pct,
        final_snapshot.error
    );
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
