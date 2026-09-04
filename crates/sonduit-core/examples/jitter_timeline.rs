//! Drive the real receiver over a synthetic link and print what it does.
//!
//! This exists because the interesting numbers are on the phone. `Telemetry`
//! carries `jitter_ms`, `buffer_target_ms` and the grow and shrink counters,
//! and the feedback report carries none of them: a desktop watching a live
//! session sees a depth and nothing that explains it. So the receiver's own
//! code is driven here instead, against an arrival timeline this file
//! generates, and the three numbers are read directly.
//!
//! The receive side is `sonduit-ffi`'s `receive_loop` reproduced: push on
//! arrival, `shed_over_budget` with both depths in hand, then hand over
//! exactly what `drain_allowance` permits. The audio callback is the only
//! thing running on its own clock. Modelling it any other way measures a
//! receiver this project does not ship -- draining the jitter buffer on the
//! device clock rather than on arrivals gives the target an influence over
//! the depth that it does not have.
//!
//! What this is not: a measurement of anybody's network. The arrival model is
//! an input, and the figures behind it are cited in
//! `docs/research/jitter-and-drift.md`. What the run measures is the
//! receiver's response to that input, which is the half that lives in this
//! repository.
//!
//! ```text
//! cargo run --release -p sonduit-core --example jitter_timeline -- \
//!     --transport wifi --seconds 300 --stall-ms 120 --stall-every 12
//! ```
//!
//! Arguments, all optional:
//!
//! | Flag | Default | Meaning |
//! | --- | --- | --- |
//! | `--transport` | `wifi` | `wifi` or `usb`, from `JitterConfig::for_transport` |
//! | `--seconds` | `300` | Length of the session |
//! | `--base-ms` | `1.5` | Mean of the per-packet exponential delay |
//! | `--stall-ms` | `120` | How long the medium is held off in one stall |
//! | `--stall-every` | `12` | Seconds between stalls, or `0` for none |
//! | `--burst-gap-ms` | `0.15` | Spacing the backlog comes out of a stall at |
//! | `--seed` | `1` | PRNG seed |
//! | `--every` | `5` | Seconds between printed lines, or `0` for none |

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::handoff::{self, Consumer, Producer};
use sonduit_core::jitter::{JitterBuffer, JitterConfig, PopOutcome, Transport};
use sonduit_core::pacing::{drain_allowance, PacingConfig};

const RATE: u64 = 48_000;
const CHANNELS: usize = 2;
const FRAMES_PER_PACKET: u32 = (PCM_PAYLOAD_BYTES / 4) as u32;
const PACKET_NANOS: u64 = FRAMES_PER_PACKET as u64 * 1_000_000_000 / RATE;
const PACKET_MS: f64 = PACKET_NANOS as f64 / 1_000_000.0;

/// What the receive thread in `sonduit-ffi` is configured with.
const PACING: PacingConfig = PacingConfig {
    floor_packets: 2,
    max_per_packet: 3,
};

/// Default gap the radio leaves between two frames it sends back to back.
///
/// A queue draining after a stall does not empty instantaneously, and packets
/// landing at literally the same nanosecond would make the wire-speed test in
/// `push` read differently from any real link. `--burst-gap-ms` overrides it,
/// because how fast the backlog comes out is the whole question: the buffer
/// only sheds a burst it can recognise as faster than the sender could have
/// produced it.
const BURST_GAP_MS: f64 = 0.15;

/// xorshift64*, so a run is reproducible from its seed without a dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in (0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64 + f64::EPSILON
    }

    /// Exponential with the given mean, the usual stand-in for a queueing
    /// delay and what puts a tail on the distribution at all.
    fn exponential(&mut self, mean: f64) -> f64 {
        -mean * self.unit().ln()
    }
}

struct Options {
    transport: Transport,
    seconds: f64,
    base_ms: f64,
    stall_ms: f64,
    stall_every_s: f64,
    burst_gap_ms: f64,
    seed: u64,
    every_s: f64,
    /// Overrides on the shipped config, so one binary can run the old
    /// constants and the new ones against the same arrival timeline.
    max_ms: Option<u32>,
    target_ms: Option<u32>,
    shrink_threshold: Option<f64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            transport: Transport::WiFi,
            seconds: 300.0,
            base_ms: 1.5,
            stall_ms: 120.0,
            stall_every_s: 12.0,
            burst_gap_ms: BURST_GAP_MS,
            seed: 1,
            every_s: 5.0,
            max_ms: None,
            target_ms: None,
            shrink_threshold: None,
        }
    }
}

fn parse() -> Options {
    let mut options = Options::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let value = arguments.next().unwrap_or_default();
        match flag.as_str() {
            "--transport" => {
                options.transport = if value == "usb" {
                    Transport::Usb
                } else {
                    Transport::WiFi
                };
            }
            "--seconds" => options.seconds = value.parse().unwrap_or(options.seconds),
            "--base-ms" => options.base_ms = value.parse().unwrap_or(options.base_ms),
            "--stall-ms" => options.stall_ms = value.parse().unwrap_or(options.stall_ms),
            "--stall-every" => {
                options.stall_every_s = value.parse().unwrap_or(options.stall_every_s);
            }
            "--burst-gap-ms" => {
                options.burst_gap_ms = value.parse().unwrap_or(options.burst_gap_ms);
            }
            "--seed" => options.seed = value.parse().unwrap_or(options.seed),
            "--every" => options.every_s = value.parse().unwrap_or(options.every_s),
            "--max-ms" => options.max_ms = value.parse().ok(),
            "--target-ms" => options.target_ms = value.parse().ok(),
            "--shrink-threshold" => options.shrink_threshold = value.parse().ok(),
            other => println!("ignoring unknown flag {other}"),
        }
    }
    options
}

/// Arrival time of every packet, in nanoseconds since the session began.
fn arrivals(options: &Options, count: usize) -> Vec<u64> {
    let mut rng = Rng(options.seed | 1);
    let mut out = Vec::with_capacity(count);
    // The radio is busy until this instant, which is what turns a stall into a
    // burst: everything produced during it lands together afterwards.
    let mut medium_free_at = 0_u64;
    let stall_nanos = (options.stall_ms * 1_000_000.0) as u64;
    let stall_period = (options.stall_every_s * 1_000_000_000.0) as u64;
    let burst_gap = (options.burst_gap_ms * 1_000_000.0) as u64;

    for index in 0..count {
        let sent = index as u64 * PACKET_NANOS;

        // A stall begins on the first packet of each period.
        let new_period = stall_period > 0
            && index > 0
            && sent / stall_period != (sent - PACKET_NANOS) / stall_period;
        if new_period {
            medium_free_at = medium_free_at.max(sent) + stall_nanos;
        }

        let delay = (rng.exponential(options.base_ms) * 1_000_000.0) as u64;
        let arrival = (sent + delay).max(medium_free_at);
        medium_free_at = arrival + burst_gap;
        out.push(arrival);
    }
    out
}

/// One receiver, played out against a timeline the caller controls.
struct Session {
    buffer: JitterBuffer,
    queue: Producer,
    device: Consumer,
    played_to: u64,
    started: bool,
    /// Frames the callback asked for and the queue could not supply.
    underrun_frames: u64,
    /// Frames handed to the callback.
    played_frames: u64,
}

impl Session {
    fn new(config: JitterConfig) -> Self {
        let format = Format::stereo_48k();
        let (queue, device) = handoff::channel(format, 400);
        Self {
            buffer: JitterBuffer::new(format, config),
            queue,
            device,
            played_to: 0,
            started: false,
            underrun_frames: 0,
            played_frames: 0,
        }
    }

    /// Let the audio callback take everything its own clock entitles it to.
    fn play_until(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.played_to);
        self.played_to = now;
        let frames = (elapsed * RATE / 1_000_000_000) as usize;
        if frames == 0 {
            return;
        }
        let mut out = vec![0_i16; frames * CHANNELS];
        let filled = self.device.fill(&mut out, frames);
        // Silence before the first packet is the buffer filling to its target,
        // which every session does once and no listener calls a dropout.
        if self.started {
            self.underrun_frames += (frames - filled) as u64;
            self.played_frames += frames as u64;
        }
        if filled > 0 {
            self.started = true;
        }
    }

    /// One arrival, handled exactly as `receive_loop` handles one.
    fn receive(&mut self, sequence: u32, arrival: u64) {
        self.play_until(arrival);
        self.buffer.push(
            sequence as u16,
            sequence.wrapping_mul(FRAMES_PER_PACKET),
            arrival,
            vec![0_u8; PCM_PAYLOAD_BYTES],
        );
        self.buffer.shed_over_budget(self.queue.queued_ms());

        let allowance = drain_allowance(self.queue.queued_ms(), PACKET_MS, PACING);
        for _ in 0..allowance {
            match self.buffer.pop() {
                PopOutcome::Packet(pcm) => {
                    if self.queue.push(&pcm) < pcm.len() {
                        break;
                    }
                }
                PopOutcome::Lost => {
                    let silence = vec![0_u8; PCM_PAYLOAD_BYTES];
                    if self.queue.push(&silence) < silence.len() {
                        break;
                    }
                }
                PopOutcome::Starved => break,
            }
        }
    }

    /// Audio the receiver is holding, across both buffers.
    fn held_ms(&self) -> f64 {
        self.buffer.depth_ms() + self.queue.queued_ms()
    }
}

/// Percentile of an already sorted slice.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn main() {
    let options = parse();
    let mut config = JitterConfig::for_transport(options.transport);
    if let Some(max_ms) = options.max_ms {
        config.max_ms = max_ms;
    }
    if let Some(target_ms) = options.target_ms {
        config.target_ms = target_ms;
    }
    if let Some(threshold) = options.shrink_threshold {
        config.shrink_threshold = threshold;
    }
    let count = (options.seconds * 1_000_000_000.0 / PACKET_NANOS as f64) as usize;
    let arrival_at = arrivals(&options, count);

    println!(
        "link={:?} target={} min={} max={} multiplier={} shrink_threshold={} shrink_cooldown={} grow_cooldown={}",
        options.transport,
        config.target_ms,
        config.min_ms,
        config.max_ms,
        config.jitter_multiplier,
        config.shrink_threshold,
        config.shrink_cooldown_packets,
        config.grow_cooldown_packets
    );
    println!(
        "packets={count} base_ms={} stall_ms={} stall_every_s={} burst_gap_ms={} seed={}",
        options.base_ms,
        options.stall_ms,
        options.stall_every_s,
        options.burst_gap_ms,
        options.seed
    );
    println!(
        "  time_s  jitter_ms  suggested  target_ms  depth_ms  held_ms  grew  shrank  underrun_ms"
    );

    let mut session = Session::new(config);
    let mut targets = Vec::with_capacity(count);
    let mut held = Vec::with_capacity(count);
    let mut suggested_peak = 0.0_f64;
    let mut jitter_peak = 0.0_f64;
    let mut next_print = 0_u64;

    for (index, arrival) in arrival_at.iter().copied().enumerate() {
        session.receive(index as u32, arrival);
        targets.push(session.buffer.target_ms());
        held.push(session.held_ms());
        suggested_peak = suggested_peak.max(session.buffer.suggested_target_ms());
        jitter_peak = jitter_peak.max(session.buffer.jitter_ms());

        if options.every_s > 0.0 && arrival >= next_print {
            let stats = session.buffer.stats();
            println!(
                "{:8.1}  {:9.2}  {:9.2}  {:9.2}  {:8.1}  {:7.1}  {:4}  {:6}  {:10.0}",
                arrival as f64 / 1e9,
                session.buffer.jitter_ms(),
                session.buffer.suggested_target_ms(),
                session.buffer.target_ms(),
                session.buffer.depth_ms(),
                session.held_ms(),
                stats.target_grew,
                stats.target_shrank,
                session.underrun_frames as f64 * 1000.0 / RATE as f64
            );
            next_print = arrival + (options.every_s * 1e9) as u64;
        }
    }

    let stats = session.buffer.stats();
    let mut sorted_held = held.clone();
    sorted_held.sort_by(f64::total_cmp);
    let peak_target = targets.iter().copied().fold(0.0_f64, f64::max);
    let second_half = &targets[targets.len() / 2..];
    let min_settled = second_half.iter().copied().fold(f64::MAX, f64::min);
    let underrun_ms = session.underrun_frames as f64 * 1000.0 / RATE as f64;
    let underrun_pct = if session.played_frames == 0 {
        0.0
    } else {
        session.underrun_frames as f64 * 100.0 / session.played_frames as f64
    };

    println!("---");
    println!(
        "SUMMARY link={:?} grew={} shrank={} target_final={:.1} target_peak={peak_target:.1} target_min_second_half={min_settled:.1} jitter_peak={jitter_peak:.1} suggested_peak={suggested_peak:.1} held_p50={:.1} held_p95={:.1} held_max={:.1} underrun_ms={underrun_ms:.0} underrun_pct={underrun_pct:.3} lost={} shed={} accepted={}",
        options.transport,
        stats.target_grew,
        stats.target_shrank,
        session.buffer.target_ms(),
        percentile(&sorted_held, 0.50),
        percentile(&sorted_held, 0.95),
        percentile(&sorted_held, 1.0),
        stats.lost,
        stats.shed,
        stats.accepted
    );
}
