//! What render endpoints does this machine have, and can each be captured?
//!
//! Run with `cargo run -p sonduit-capture-win --example endpoints` to list
//! them, or pass an index to open capture on that one:
//!
//! ```text
//! cargo run -p sonduit-capture-win --example endpoints -- 1
//! ```
//!
//! Selection is the part worth checking by hand. Enumeration returning three
//! names proves nothing about whether asking for the second one actually taps
//! the second one, and the failure mode -- silence from a device the user is
//! not listening to -- looks identical to a working bridge from the outside.
//! Opening reports the endpoint that was really opened, so a fallback shows up
//! as a different name rather than as a puzzle.
//!
//! `-- sta` checks the other claim the desktop relies on: that enumeration
//! cannot run on a thread that is already in a single-threaded apartment,
//! which is what the thread hosting a webview is. The desktop spawns a thread
//! for this on the strength of that, so it is worth being a fact rather than
//! folklore.

#[cfg(windows)]
fn main() {
    use sonduit_capture_win::{enumerate_endpoints, open, CaptureMode};

    let endpoints = match enumerate_endpoints() {
        Ok(endpoints) => endpoints,
        Err(error) => {
            println!("enumerate failed: {error}");
            return;
        }
    };

    println!("endpoints: {}", endpoints.len());
    for (index, endpoint) in endpoints.iter().enumerate() {
        let marker = if endpoint.is_default { "*" } else { " " };
        println!("  [{index}] {marker} {}", endpoint.name);
        println!("          {}", endpoint.id);
    }

    let Some(argument) = std::env::args().nth(1) else {
        println!("\npass an index to open one, `missing` for the fallback, `sta` for apartments");
        return;
    };

    if argument == "sta" {
        apartment_check();
        return;
    }

    // A deliberately bogus id stands in for the device that was unplugged
    // between the settings page listing it and the session starting.
    let requested = if argument == "missing" {
        Some("{0.0.0.00000000}.{deadbeef-0000-0000-0000-000000000000}".to_string())
    } else {
        argument
            .parse::<usize>()
            .ok()
            .and_then(|index| endpoints.get(index))
            .map(|endpoint| endpoint.id.clone())
    };

    let Some(requested) = requested else {
        println!("\n{argument} is not one of the indexes above");
        return;
    };

    println!("\nrequested: {requested}");

    let mut capture = match open(CaptureMode::EndpointLoopback, 10, Some(&requested)) {
        Ok(capture) => capture,
        Err(error) => {
            println!("open failed: {error}");
            return;
        }
    };

    let opened = capture.endpoint().clone();
    println!("opened:    {} ({})", opened.name, opened.id);
    if opened.id == requested {
        println!("RESULT: the requested endpoint is the one being captured");
    } else {
        println!(
            "RESULT: fell back to the default; the panel would say {}",
            opened.name
        );
    }

    let format = capture.format();
    println!(
        "format: {} Hz, {} ch, {} bit",
        format.sample_rate,
        format.channels,
        format.bit_depth.bits()
    );

    // Draining for a couple of seconds answers the question the user actually
    // has, which is not "did it open" but "is the audio I can hear the audio
    // being sent". A chosen endpoint nothing is playing to opens, clocks and
    // hands back frames exactly like a working one; the peak is the only thing
    // that tells them apart, so play something on the endpoint being tested.
    let mut buffer = Vec::new();
    let mut frames = 0_usize;
    let mut peak = 0_i16;
    let start = std::time::Instant::now();

    while start.elapsed().as_secs() < 2 {
        buffer.clear();
        match capture.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                frames += count;
                for pair in buffer.chunks_exact(2) {
                    peak = peak.max(i16::from_le_bytes([pair[0], pair[1]]).saturating_abs());
                }
            }
            Err(error) => {
                println!("read failed: {error}");
                return;
            }
        }
    }

    println!(
        "frames: {frames} in {:.2}s wall",
        start.elapsed().as_secs_f64()
    );
    if peak == 0 {
        println!("peak:   0 (silence -- nothing is playing to this endpoint)");
    } else {
        println!("peak:   {peak}");
    }
}

/// Enumerate from a thread that is already an STA, then from a spawned one.
///
/// The first stands in for the thread a Tauri command runs on. The second is
/// what `bridge::endpoints` does instead, and the difference between the two
/// results is the entire reason that function spawns anything.
#[cfg(windows)]
fn apartment_check() {
    use sonduit_capture_win::enumerate_endpoints;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

    // SAFETY: entering an apartment on this thread before it has made any COM
    // call. Deliberately never balanced: the process exits from here, and
    // uninitialising is exactly what must not happen to a webview's thread.
    let entered = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    println!("this thread is now an STA: {}", entered.is_ok());

    match enumerate_endpoints() {
        Ok(found) => println!(
            "on the STA thread: unexpectedly returned {} endpoints",
            found.len()
        ),
        Err(error) => println!("on the STA thread: refused, as expected -- {error}"),
    }

    let spawned = std::thread::spawn(enumerate_endpoints)
        .join()
        .expect("the listing thread should not panic");
    match spawned {
        Ok(found) => println!("on a spawned thread: {} endpoints", found.len()),
        Err(error) => println!("on a spawned thread: failed -- {error}"),
    }
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
