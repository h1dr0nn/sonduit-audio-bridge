//! Does replacing a live capture client actually work?
//!
//! The recovery path opens a new client and assigns it over the old one, so
//! for a moment both exist and both hold the same endpoint. Shared-mode
//! loopback is supposed to allow that; this checks it rather than assuming it,
//! because the failure mode is a bridge that dies the first time somebody
//! unplugs a headset.
//!
//! Run with `cargo run -p sonduit-desktop --example reopen`.

#[cfg(windows)]
fn main() {
    use sonduit_capture_win::{open, CaptureMode};
    use std::time::Instant;

    fn drain(capture: &mut sonduit_capture_win::LoopbackCapture, seconds: u64) -> usize {
        let mut buffer = Vec::new();
        let mut frames = 0;
        let start = Instant::now();
        while start.elapsed().as_secs() < seconds {
            buffer.clear();
            match capture.read(&mut buffer) {
                Ok(count) => frames += count,
                Err(error) => {
                    println!("read failed: {error}");
                    return frames;
                }
            }
        }
        frames
    }

    let mut capture = match open(CaptureMode::EndpointLoopback, 10, None) {
        Ok(capture) => capture,
        Err(error) => {
            println!("open failed: {error}");
            return;
        }
    };
    let before = drain(&mut capture, 1);
    println!("before reopen: {before} frames");

    // Exactly what the recovery path does: open the replacement, then let the
    // assignment drop the old one.
    let replacement = match open(CaptureMode::EndpointLoopback, 10, None) {
        Ok(replacement) => replacement,
        Err(error) => {
            println!("RESULT: a second client could not be opened: {error}");
            return;
        }
    };
    capture = replacement;
    println!("reopened while the first client was still alive");

    let after = drain(&mut capture, 1);
    println!("after reopen:  {after} frames");

    if after == 0 {
        println!("RESULT: the reopened client produces nothing");
    } else if after < before / 2 {
        println!("RESULT: the reopened client is producing far less than the first");
    } else {
        println!("RESULT: ok");
    }
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
