//! Drive one real session against a real phone, without the window.
//!
//! Written for a verification pass: the window is driven by another agent in
//! this session and the single-instance plugin means only one copy of the
//! application can run at a time. This takes the same path the button does --
//! `bridge::discover`, `BridgeState::remember`, `bridge::start` -- and polls
//! the snapshot fast enough to time a link change, which nothing watching the
//! webview at four frames a second can do.
//!
//! `cargo run -p sonduit-desktop --bin verify_session -- <code> <ip:port> <seconds>`
//!
//! It lives under `src/bin` rather than `examples/` because `tauri-build`
//! emits its link arguments with `cargo:rustc-link-arg-bins`, which does not
//! reach an example target: an example that builds a Tauri app links but will
//! not load, exiting 0xC0000139 before `main` runs.

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};

    use sonduit_desktop::bridge::{self, BridgeState, StartOptions};
    use sonduit_transport::pairing::PairingCode;

    println!("boot");
    let mut arguments = std::env::args().skip(1);
    let code = arguments.next().unwrap_or_default();
    let target = arguments.next().unwrap_or_default();
    let seconds: u64 = arguments.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let preference = arguments.next().unwrap_or_else(|| "auto".to_string());

    let Some(credential) = PairingCode::parse(&code) else {
        println!("usage: verify_session <six-digit-code> <ip:port> [seconds] [auto|wifi|usb]");
        return;
    };

    // A real handle, because `bridge::start` takes one and the telemetry
    // thread emits through it. No windows: the configuration's window is
    // dropped from the context before the app is built, so this neither
    // paints anything nor collides with a copy that does.
    println!("building context");
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .build(context)
        .expect("the app must build");
    let handle = app.handle().clone();
    println!("app built");

    let started = Instant::now();
    // Both clocks: the monotonic one for durations inside this run, and the
    // wall clock so the lines can be laid beside logcat and beside a watcher
    // timing when the tether interface disappears.
    let epoch = |()| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |since| since.as_secs_f64())
    };
    let stamp = move |at: Instant| {
        let _ = at;
        format!(
            "{:9.4} {:.4}",
            at.duration_since(started).as_secs_f64(),
            epoch(())
        )
    };

    println!("[{}] scanning for {code}", stamp(Instant::now()));
    let found = match bridge::discover(&code) {
        Ok(found) => found,
        Err(error) => {
            println!("scan failed: {error}");
            return;
        }
    };
    for paired in &found {
        println!(
            "[{}] found {} at {}",
            stamp(Instant::now()),
            paired.device.name,
            paired.device.id
        );
    }
    if found.is_empty() {
        println!("nothing answered the probe");
        return;
    }

    // Pairing over one interface, then remembering the phone at its address on
    // another.
    //
    // Needed because this network puts the phone's Wi-Fi address
    // (10.10.22.160/22) on a different subnet from the desktop's Ethernet
    // (10.10.0.61/22), and `discover` reaches a phone only by limited
    // broadcast or by a tether gateway -- neither of which crosses a router.
    // So the phone can be paired with over the cable and nowhere else, and a
    // Wi-Fi session could never be started here at all.
    //
    // The secret does not care which interface agreed it: the receiver holds
    // one session key for this desktop and its audio port is the same on every
    // interface. Rewriting the address the pairing is filed under is therefore
    // the same session the user would have got by pairing over Wi-Fi, minus
    // the QR code that this pass has no way to hold up to a camera.
    let mut found = found;
    if !target.is_empty() && !found.iter().any(|paired| paired.device.id == target) {
        println!(
            "[{}] refiling the pairing from {} to {target}",
            stamp(Instant::now()),
            found[0].device.id
        );
        found[0].device.id = target.clone();
        found[0].device.address = target.clone();
    }

    let state = BridgeState::default();
    state.remember(&found, &credential);

    let chosen = if target.is_empty() {
        found[0].device.id.clone()
    } else {
        target.clone()
    };
    println!("[{}] starting to {chosen}", stamp(Instant::now()));

    let info = match bridge::start(
        &handle,
        &state,
        StartOptions {
            target: Some(chosen),
            bind: None,
            scream_compatible: false,
            preferred_transport: Some(preference),
            capture_device_id: None,
        },
    ) {
        Ok(info) => info,
        Err(error) => {
            println!("start failed: {error}");
            return;
        }
    };
    println!(
        "[{}] session up: endpoint={:?} {} Hz {} ch {} bit target={} transport={} wire={} encrypted={}",
        stamp(Instant::now()),
        info.endpoint,
        info.sample_rate,
        info.channels,
        info.bit_depth,
        info.target,
        info.transport,
        info.wire,
        info.encrypted
    );

    // Five milliseconds, so a retreat that is claimed to take fifty is timed
    // to a tenth of its own size.
    let tick = Duration::from_millis(5);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last_route = String::new();
    let mut last_print = Instant::now();

    while Instant::now() < deadline {
        std::thread::sleep(tick);
        let now = Instant::now();
        let snapshot = state.snapshot();
        let route = snapshot
            .session
            .as_ref()
            .map_or_else(String::new, |session| {
                format!("{} via {}", session.target, session.transport)
            });
        if route != last_route {
            println!(
                "[{}] ROUTE {route}  (status={} sent={:?} loss={:?})",
                stamp(now),
                snapshot.status,
                snapshot.telemetry.packets_sent,
                snapshot.telemetry.packet_loss_pct
            );
            last_route = route;
        }
        if now.duration_since(last_print) >= Duration::from_secs(1) {
            last_print = now;
            println!(
                "[{}] status={} sent={:?} recv={:?} loss={:?} rtt={:?} depth={:?} audio_s={:?} refused_reports={:?} err={:?}",
                stamp(now),
                snapshot.status,
                snapshot.telemetry.packets_sent,
                snapshot.telemetry.packets_received,
                snapshot.telemetry.packet_loss_pct,
                snapshot.telemetry.round_trip_ms,
                snapshot.telemetry.buffer_depth_ms,
                snapshot.telemetry.audio_seconds,
                snapshot.telemetry.refused_reports,
                snapshot.error
            );
        }
    }

    let _ = bridge::stop(&state);
    let final_snapshot = state.snapshot();
    println!(
        "[{}] stopped: sent={:?} recv={:?} loss={:?} audio_s={:?}",
        stamp(Instant::now()),
        final_snapshot.telemetry.packets_sent,
        final_snapshot.telemetry.packets_received,
        final_snapshot.telemetry.packet_loss_pct,
        final_snapshot.telemetry.audio_seconds
    );
    drop(app);
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
