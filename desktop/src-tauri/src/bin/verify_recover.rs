//! Break a running session's route, let it retreat, and watch what the panel
//! says afterwards.
//!
//! The defect this drives out: `Accumulator::last_error` was set by the send
//! path and cleared by nothing on it, and the telemetry tick republished it
//! four times a second. A session that retreated from a pulled cable onto
//! Wi-Fi went on reporting
//! `socket error: The requested address is not valid in its context. (os error
//! 10049)` for the rest of its life, while audio was arriving at the phone the
//! whole time.
//!
//! Reproducing that needs a route that fails the way an interface that has
//! gone fails, and a way back. So this drives the product's own
//! `capture_and_follow` against the phone over Wi-Fi, arms that route as the
//! standby, and then offers it a live route it cannot send on: `0.0.0.0` is
//! refused by Windows with WSAEADDRNOTAVAIL, which is os error 10049 -- the
//! same error, from the same call, as the cable coming out. The send loop
//! counts five refusals, takes the standby, and carries on. Everything after
//! the offer is the product deciding what to do.
//!
//! The audio is cleartext, for the same reason `verify_follow` sends
//! cleartext: `bridge::start` seals every Sonduit session against a key this
//! driver has no pairing to agree, so nothing the phone received would ever be
//! played and the session under test would not be a session.
//!
//! `hold` is how long the session is left with nowhere to go, so both halves
//! of the rule can be seen in one run: while it is broken the message must be
//! on every telemetry tick, and the moment the standby is armed and taken it
//! must be gone. A hold of zero exercises the other case, a failure that heals
//! inside one tick and is never shown at all -- for which the count of send
//! failures is the only record, and has to be there.
//!
//! `cargo run -p sonduit-desktop --bin verify_recover -- <phone-ip> <seconds> <break-at> <hold>`
//!
//! It lives under `src/bin` rather than `examples/` because `tauri-build`
//! emits its link arguments with `cargo:rustc-link-arg-bins`, which does not
//! reach an example target: an example that builds a Tauri app links but will
//! not load, exiting 0xC0000139 before `main` runs.

#[cfg(windows)]
fn main() {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use sonduit_capture_win::{open, CaptureMode};
    use sonduit_desktop::bridge::{
        capture_and_follow, BridgeSnapshot, Link, LinkKind, LinkSwitch, Route,
    };
    use sonduit_transport::{Wire, DEFAULT_PORT};

    let mut arguments = std::env::args().skip(1);
    let phone: Ipv4Addr = arguments
        .next()
        .and_then(|text| text.parse().ok())
        .expect("an address the phone is listening on");
    let seconds: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let break_at: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let hold: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(4);

    let started = Instant::now();
    let stamp = move || format!("{:7.3}s", started.elapsed().as_secs_f64());

    let home = Route::unbound(
        SocketAddr::new(phone.into(), DEFAULT_PORT),
        LinkKind::Wireless,
    );
    let opening = Link::bind(home.clone()).expect("the route to the phone must bind");
    let switch = Arc::new(LinkSwitch::new(home.clone()));
    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(BridgeSnapshot::default()));

    let mut capture = open(CaptureMode::EndpointLoopback, 10, None).expect("capture must open");
    let format = capture.format();
    let stopper = capture.stopper();
    println!(
        "[{}] capturing {} Hz {} ch, sending cleartext sonduit to {}",
        stamp(),
        format.sample_rate,
        format.channels,
        home.target
    );

    // The stand-in watcher: arm the way back, then hand the send loop a route
    // that cannot send. Nothing after this tells the loop what to do.
    {
        let switch = Arc::clone(&switch);
        let stop = Arc::clone(&stop);
        let home = home.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(break_at));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // Unspecified as a destination is refused by the socket with os
            // error 10049, which is what an interface that has gone gives.
            let dead = Route::unbound(
                SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_PORT)),
                LinkKind::Wired,
            );
            match Link::bind(dead) {
                Ok(link) => {
                    println!("[{}] OFFER a route that cannot send", stamp());
                    switch.offer(link);
                }
                Err(error) => println!("[{}] could not bind the dead route: {error}", stamp()),
            }

            // Nothing to retreat to until now, so the session stays broken and
            // has to keep saying so.
            std::thread::sleep(Duration::from_secs(hold));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match Link::bind(home) {
                Ok(link) => {
                    println!("[{}] ARM the way back", stamp());
                    switch.arm(link);
                }
                Err(error) => println!("[{}] could not arm: {error}", stamp()),
            }
        });
    }

    // What the panel would be showing, sampled faster than the telemetry
    // thread emits so a message that comes and goes is not missed.
    {
        let stop = Arc::clone(&stop);
        let snapshot = Arc::clone(&snapshot);
        let switch = Arc::clone(&switch);
        std::thread::spawn(move || {
            let mut last = String::new();
            let mut last_print = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
                let current = match snapshot.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => continue,
                };
                let route = switch
                    .live()
                    .map_or_else(String::new, |route| route.target.to_string());
                let line = format!(
                    "status={} route={route} sent={:?} send_failures={:?} err={:?}",
                    current.status,
                    current.telemetry.packets_sent,
                    current.telemetry.send_failures,
                    current.error
                );
                let key = format!("{} {:?}", current.status, current.error);
                if key != last {
                    println!("[{}] CHANGE {line}", stamp());
                    last = key;
                    last_print = Instant::now();
                } else if last_print.elapsed() >= Duration::from_secs(1) {
                    last_print = Instant::now();
                    println!("[{}]        {line}", stamp());
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
        "[{}] finished: sent={:?} send_failures={:?} err={:?}",
        stamp(),
        final_snapshot.telemetry.packets_sent,
        final_snapshot.telemetry.send_failures,
        final_snapshot.error
    );
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
