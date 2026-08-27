//! Android audio playback.
//!
//! The AAudio binding is Android-only; everything that decides *what* to play
//! lives here and compiles anywhere, so the interesting logic is tested on the
//! development machine rather than only on a device.
//!
//! ADR-003 records why this targets `ndk::audio` rather than the `oboe` crate.

use std::sync::atomic::{AtomicU64, Ordering};

use sonduit_core::conceal::PacketConcealer;
use sonduit_core::format::Format;
use sonduit_core::jitter::{JitterBuffer, PopOutcome};

#[cfg(target_os = "android")]
pub mod aaudio;

#[cfg(target_os = "android")]
pub use aaudio::Playback;

/// What the device actually granted, which is often not what was requested.
///
/// AAudio does not fail an open when it cannot honour the requested sharing or
/// performance mode; it succeeds with something worse. Every field here must
/// be read back from the opened stream rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantedStream {
    /// Frames the device processes at once. Buffer size must be a multiple.
    pub frames_per_burst: u32,
    /// Whether exclusive sharing mode was granted.
    pub exclusive: bool,
    /// Whether low-latency performance mode was granted.
    pub low_latency: bool,
    /// Whether the MMAP data path is in use.
    pub mmap: bool,
    /// Format the stream actually runs at.
    pub format: Format,
}

/// Playback failures.
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    /// No AAudio implementation on this target.
    #[error("android playback is unavailable on this platform")]
    Unsupported,

    /// The stream was disconnected, for instance by a headset being unplugged.
    ///
    /// A disconnected stream cannot be reused. It must be closed and a new one
    /// opened, and the new one may have a different burst size and rate.
    #[error("audio stream disconnected")]
    Disconnected,

    /// An AAudio call failed.
    #[error("aaudio error: {0}")]
    Platform(String),
}

/// Something the audio callback can pull PCM from.
///
/// `fill` runs on a realtime thread. It must not allocate, must not block, and
/// must never call into the JVM: a GC pause inside an audio callback is an
/// audible dropout.
pub trait PcmSource: Send {
    /// Fill `out` with interleaved samples for `frames` frames.
    ///
    /// Returns the frames actually written. Writing fewer than asked for is
    /// allowed; the caller conceals the remainder with silence.
    fn fill(&mut self, out: &mut [i16], frames: usize) -> usize;
}

/// What the audio callback actually holds.
///
/// Separate from [`PcmSource`] because the callback has shared access, not
/// exclusive access: the receive thread is writing into the same buffer. The
/// blanket implementation below supplies the only sound way to bridge the two,
/// which is a lock the callback refuses to wait on.
pub trait CallbackSource: Send + Sync {
    /// Fill `out` with interleaved samples, returning the frames written.
    ///
    /// Must return promptly. Returning zero is always allowed and is heard as
    /// a brief dropout, which is far better than the dropout caused by making
    /// a realtime thread wait.
    fn fill(&self, out: &mut [i16], frames: usize) -> usize;
}

impl<T: PcmSource> CallbackSource for std::sync::Mutex<T> {
    fn fill(&self, out: &mut [i16], frames: usize) -> usize {
        // try_lock, not lock. Blocking a realtime thread on a normal-priority
        // thread's work is the priority inversion this whole design exists to
        // avoid; a missed block is silence, an inverted one is a stall.
        match self.try_lock() {
            Ok(mut source) => source.fill(out, frames),
            Err(_) => 0,
        }
    }
}

/// Counters the audio callback maintains, read by the UI thread.
///
/// Atomics rather than a mutex: the UI reading telemetry must never be able to
/// make the audio callback wait.
#[derive(Debug, Default)]
pub struct PlaybackCounters {
    /// Frames handed to the device.
    pub frames_played: AtomicU64,
    /// Frames of silence written because the source had nothing.
    pub frames_underrun: AtomicU64,
    /// Times the callback ran.
    pub callbacks: AtomicU64,
}

impl PlaybackCounters {
    /// Frames played so far.
    #[must_use]
    pub fn frames_played(&self) -> u64 {
        self.frames_played.load(Ordering::Relaxed)
    }

    /// Frames of silence emitted to cover an empty source.
    #[must_use]
    pub fn frames_underrun(&self) -> u64 {
        self.frames_underrun.load(Ordering::Relaxed)
    }

    /// Fraction of played frames that were concealment, in percent.
    #[must_use]
    pub fn underrun_percent(&self) -> f64 {
        let played = self.frames_played();
        if played == 0 {
            return 0.0;
        }
        self.frames_underrun() as f64 * 100.0 / played as f64
    }
}

/// Drains a jitter buffer into the audio callback.
///
/// The buffer deals in whole packets and the callback asks for whatever the
/// device's burst size happens to be, which is never the same number. The
/// leftover of a partly consumed packet is carried here; without it every
/// callback would either discard the tail of a packet or stall waiting for a
/// whole one.
pub struct JitterSource {
    buffer: JitterBuffer,
    format: Format,
    /// The last packet that actually arrived.
    residue: Vec<u8>,
    /// Synthesises a replacement for a packet that did not.
    ///
    /// Held rather than built per loss because everything it needs is
    /// preallocated, and because it has to see the audio that did arrive in
    /// order to continue it.
    concealer: PacketConcealer,
    /// Whether the callback is currently draining concealment or real audio.
    ///
    /// The two live in separate buffers so that neither path has to copy a
    /// packet or resize anything while the device is waiting.
    from_concealer: bool,
    /// How far into the current packet the callback has read.
    offset: usize,
    /// Frames of concealment emitted to cover packets that never arrived.
    concealed: u64,
}

impl JitterSource {
    /// Wrap a jitter buffer.
    #[must_use]
    pub fn new(buffer: JitterBuffer, format: Format) -> Self {
        Self {
            buffer,
            format,
            residue: Vec::new(),
            concealer: PacketConcealer::new(format),
            from_concealer: false,
            offset: 0,
            concealed: 0,
        }
    }

    /// The buffer being drained, for pushing arriving packets into.
    pub fn buffer_mut(&mut self) -> &mut JitterBuffer {
        &mut self.buffer
    }

    /// The buffer being drained, for reading statistics.
    #[must_use]
    pub const fn buffer(&self) -> &JitterBuffer {
        &self.buffer
    }

    /// Frames of concealment emitted because a packet never arrived.
    ///
    /// Distinct from an underrun: this is loss the network caused, not the
    /// application failing to keep up.
    #[must_use]
    pub const fn concealed_frames(&self) -> u64 {
        self.concealed
    }

    /// The bytes the callback is currently reading from.
    fn current(&self) -> &[u8] {
        if self.from_concealer {
            self.concealer.packet()
        } else {
            &self.residue
        }
    }

    /// Pull the next packet, or a packet's worth of concealment.
    ///
    /// Returns false when there is nothing to play at all, which is the
    /// difference between "the sender stopped" and "one packet was lost".
    fn refill(&mut self) -> bool {
        match self.buffer.pop() {
            PopOutcome::Packet(pcm) => {
                // The concealer has to see what played in order to continue it
                // when the next packet does not arrive.
                self.concealer.observe(&pcm);
                self.residue = pcm;
                self.from_concealer = false;
                self.offset = 0;
                true
            }
            PopOutcome::Lost => {
                // Exactly one packet of concealment, never more and never
                // less, keeps the timeline aligned. Skipping the gap instead
                // would shorten playback by a packet and shift everything
                // after it earlier, which the drift estimator would then chase
                // as a step that never happened on the sender.
                self.concealer.conceal();
                self.from_concealer = true;
                self.offset = 0;
                self.concealed += self.concealer.frames() as u64;
                true
            }
            PopOutcome::Starved => false,
        }
    }
}

impl PcmSource for JitterSource {
    fn fill(&mut self, out: &mut [i16], frames: usize) -> usize {
        let channels = self.format.channels as usize;
        let wanted = frames * channels;
        let mut written = 0;

        while written < wanted {
            if self.offset >= self.current().len() && !self.refill() {
                break;
            }

            let available = (self.current().len() - self.offset) / 2;
            let take = available.min(wanted - written);
            if take == 0 {
                break;
            }

            let offset = self.offset;
            let source = self.current();
            for index in 0..take {
                let at = offset + index * 2;
                out[written + index] = i16::from_le_bytes([source[at], source[at + 1]]);
            }

            self.offset += take * 2;
            written += take;
        }

        written / channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonduit_core::format::PCM_PAYLOAD_BYTES;
    use sonduit_core::jitter::JitterConfig;

    fn source() -> JitterSource {
        let format = Format::stereo_48k();
        let config = JitterConfig {
            // One packet of target depth, so tests do not have to push thirty
            // milliseconds of audio before anything plays.
            target_ms: 6,
            min_ms: 6,
            ..JitterConfig::default()
        };
        JitterSource::new(JitterBuffer::new(format, config), format)
    }

    /// A packet whose every sample is `value`.
    fn packet(value: i16) -> Vec<u8> {
        value
            .to_le_bytes()
            .iter()
            .copied()
            .cycle()
            .take(PCM_PAYLOAD_BYTES)
            .collect()
    }

    fn push(source: &mut JitterSource, sequence: u16, value: i16) {
        let frames = (PCM_PAYLOAD_BYTES / 4) as u32;
        source
            .buffer_mut()
            .push(sequence, u32::from(sequence) * frames, 0, packet(value));
    }

    #[test]
    fn an_empty_buffer_produces_no_frames_rather_than_noise() {
        // The callback fills the rest with silence. Returning a frame count
        // that was never written would leave the device playing whatever was
        // in the buffer.
        let mut source = source();
        let mut out = [0_i16; 256];
        assert_eq!(source.fill(&mut out, 128), 0);
    }

    #[test]
    fn a_callback_smaller_than_a_packet_leaves_the_rest_for_the_next_one() {
        // The device burst size is never a whole packet, so this is the normal
        // case rather than an edge one.
        let mut source = source();
        push(&mut source, 0, 1000);

        let mut out = [0_i16; 64];
        assert_eq!(source.fill(&mut out, 32), 32);
        assert!(out.iter().all(|&sample| sample == 1000));

        // The rest of the same packet is still there.
        let mut out = [0_i16; 64];
        assert_eq!(source.fill(&mut out, 32), 32);
        assert!(out.iter().all(|&sample| sample == 1000));
    }

    #[test]
    fn a_callback_larger_than_a_packet_spans_packets_without_a_gap() {
        let mut source = source();
        push(&mut source, 0, 1000);
        push(&mut source, 1, 2000);

        let frames_per_packet = PCM_PAYLOAD_BYTES / 4;
        let mut out = vec![0_i16; frames_per_packet * 2 * 2];
        let written = source.fill(&mut out, frames_per_packet + 10);

        assert_eq!(written, frames_per_packet + 10);
        assert_eq!(out[0], 1000, "starts in the first packet");
        assert_eq!(
            out[frames_per_packet * 2],
            2000,
            "continues straight into the second"
        );
    }

    #[test]
    fn a_lost_packet_becomes_a_packet_of_concealment_not_a_skip() {
        // Skipping would shorten the timeline by a packet and shift everything
        // after it earlier, which the drift estimator would then chase. The
        // gap is filled by continuing the audio before it rather than by
        // muting, so on this constant-valued stream it stays at the level it
        // was at instead of stepping to zero and back.
        let mut source = source();
        push(&mut source, 0, 1000);
        push(&mut source, 2, 3000);

        let frames_per_packet = PCM_PAYLOAD_BYTES / 4;
        let mut out = vec![0_i16; frames_per_packet * 3 * 2];
        let written = source.fill(&mut out, frames_per_packet * 3);

        assert_eq!(written, frames_per_packet * 3);
        assert_eq!(out[0], 1000);

        let gap = &out[frames_per_packet * 2..frames_per_packet * 4];
        assert!(
            gap.iter().all(|&sample| sample == 1000),
            "the gap should continue the audio, not mute it"
        );
        assert_eq!(out[frames_per_packet * 4], 3000, "and then packet two");
        assert_eq!(source.concealed_frames(), frames_per_packet as u64);
    }

    #[test]
    fn concealment_does_not_leak_into_the_packet_that_follows_it() {
        // Concealment and received audio live in different buffers, and a
        // stale offset or flag would replay part of the synthesised packet
        // over the real one that arrived after it.
        let mut source = source();
        push(&mut source, 0, 1000);
        push(&mut source, 2, 3000);
        push(&mut source, 3, 4000);

        let frames_per_packet = PCM_PAYLOAD_BYTES / 4;
        let mut out = vec![0_i16; frames_per_packet * 4 * 2];
        assert_eq!(
            source.fill(&mut out, frames_per_packet * 4),
            frames_per_packet * 4
        );

        let third = &out[frames_per_packet * 4..frames_per_packet * 6];
        assert!(third.iter().all(|&sample| sample == 3000));
        let fourth = &out[frames_per_packet * 6..];
        assert!(fourth.iter().all(|&sample| sample == 4000));
    }

    #[test]
    fn samples_are_read_back_in_the_byte_order_they_were_sent() {
        // Getting this wrong is inaudible on a constant tone and catastrophic
        // on anything else, so it is asserted explicitly.
        let mut source = source();
        let frames = (PCM_PAYLOAD_BYTES / 4) as u32;
        let mut pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
        pcm[0] = 0x34;
        pcm[1] = 0x12;
        source.buffer_mut().push(0, 0, 0, pcm);

        let mut out = [0_i16; 2];
        source.fill(&mut out, 1);
        assert_eq!(out[0], 0x1234);
        let _ = frames;
    }

    #[test]
    fn underrun_percent_is_zero_before_anything_plays() {
        let counters = PlaybackCounters::default();
        assert_eq!(counters.underrun_percent(), 0.0);
    }

    #[test]
    fn underrun_percent_is_the_share_of_concealed_frames() {
        let counters = PlaybackCounters::default();
        counters.frames_played.store(1_000, Ordering::Relaxed);
        counters.frames_underrun.store(25, Ordering::Relaxed);
        assert_eq!(counters.underrun_percent(), 2.5);
    }
}
