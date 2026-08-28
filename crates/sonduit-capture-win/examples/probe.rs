//! Prove WASAPI loopback works on this machine.
//!
//! Run with `cargo run -p sonduit-capture-win --example probe`. Play something
//! while it runs: it reports the engine format, how many frames arrived, and
//! the peak level, so silence caused by a stalled stream is distinguishable
//! from silence caused by nothing playing.

#[cfg(windows)]
fn main() {
    use sonduit_capture_win::{enumerate_endpoints, open, CaptureMode};
    use std::time::Instant;

    match enumerate_endpoints() {
        Ok(endpoints) => {
            println!("endpoints: {}", endpoints.len());
            for endpoint in &endpoints {
                let marker = if endpoint.is_default { "*" } else { " " };
                println!("  {marker} {}", endpoint.name);
            }
        }
        Err(error) => println!("enumerate failed: {error}"),
    }

    let mut capture = match open(CaptureMode::EndpointLoopback, 10, None) {
        Ok(capture) => capture,
        Err(error) => {
            println!("open failed: {error}");
            return;
        }
    };

    let format = capture.format();
    println!(
        "format: {} Hz, {} ch, {} bit",
        format.sample_rate,
        format.channels,
        format.bit_depth.bits()
    );

    let mut buffer = Vec::new();
    let mut frames = 0_usize;
    let mut peak = 0_i16;
    let mut blocks = 0_usize;
    let start = Instant::now();

    while start.elapsed().as_secs() < 3 {
        buffer.clear();
        match capture.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                frames += count;
                blocks += 1;
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

    let seconds = frames as f64 / f64::from(format.sample_rate);
    println!("blocks: {blocks}");
    println!(
        "frames: {frames} ({seconds:.2}s of audio in {:.2}s wall)",
        start.elapsed().as_secs_f64()
    );
    println!(
        "peak:   {peak} ({:.1} dBFS)",
        20.0 * f64::from(peak.max(1)).log10() - 90.3
    );

    if frames == 0 {
        println!("RESULT: no audio. The keepalive is not clocking the engine.");
    } else if seconds < start.elapsed().as_secs_f64() * 0.9 {
        println!("RESULT: gaps. Captured less audio than wall time.");
    } else {
        println!("RESULT: ok");
    }
}

#[cfg(not(windows))]
fn main() {
    println!("windows only");
}
