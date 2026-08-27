//! AAudio output.
//!
//! # What AAudio does not tell you
//!
//! Opening a stream with `SharingMode::Exclusive` and
//! `PerformanceMode::LowLatency` does not fail when the device cannot honour
//! either one. It succeeds, quietly, with shared mode and no low-latency path,
//! and the only way to find out is to read the values back off the opened
//! stream. Everything in [`GrantedStream`] is read back for that reason.
//!
//! The same applies to the burst size. `frames_per_burst` is a property of the
//! device and the granted path, not of the request, and the buffer size has to
//! be a multiple of it or every write straddles a burst boundary and costs an
//! extra period of latency. It is read back and used, never assumed.
//!
//! # The callback
//!
//! Audio is pulled by AAudio on a thread it owns, at realtime priority. That
//! callback must not allocate, must not lock anything another thread holds for
//! long, and must never call into the JVM: a GC pause inside an audio callback
//! is an audible dropout. The callback here reads from a lock-free-enough
//! source guarded by a mutex that no long operation ever holds, and does
//! nothing else.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use ndk::audio::{
    AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode, AudioSharingMode,
    AudioStream, AudioStreamBuilder,
};
use sonduit_core::format::Format;

use crate::{CallbackSource, GrantedStream, PlaybackCounters, PlaybackError};

/// A running AAudio output stream.
pub struct Playback {
    stream: AudioStream,
    granted: GrantedStream,
    counters: Arc<PlaybackCounters>,
}

impl Playback {
    /// Open an output stream and start it.
    ///
    /// `format` is what the sender is producing. AAudio is asked for exactly
    /// that: resampling here would add latency and a failure mode, and the
    /// device usually runs at 48 kHz anyway, which is what Windows mixes at.
    ///
    /// # Errors
    /// Returns [`PlaybackError::Platform`] when the stream cannot be opened or
    /// started.
    pub fn open(format: Format, source: Arc<dyn CallbackSource>) -> Result<Self, PlaybackError> {
        let counters = Arc::new(PlaybackCounters::default());
        let channels = i32::from(format.channels);

        let callback_counters = Arc::clone(&counters);
        let callback_source = Arc::clone(&source);

        let stream = AudioStreamBuilder::new()
            .map_err(|error| PlaybackError::Platform(format!("builder: {error}")))?
            .direction(AudioDirection::Output)
            // Usage and capture policy are deliberately not set. Both default
            // to what this app wants, MEDIA and ALLOW_CAPTURE_BY_ALL, and the
            // setters arrived in API 28 and 29 respectively, above the minSdk
            // ADR-003 sets.
            .sample_rate(format.sample_rate as i32)
            .channel_count(channels)
            .format(AudioFormat::PCM_I16)
            .sharing_mode(AudioSharingMode::Exclusive)
            .performance_mode(AudioPerformanceMode::LowLatency)
            .data_callback(Box::new(move |_stream, data, frames| {
                let frames = frames.max(0) as usize;
                let samples = frames * channels as usize;

                // SAFETY: AAudio guarantees `data` points at `frames *
                // channels` samples of the format the stream was opened with,
                // which is PCM_I16, and that the buffer is writable for the
                // duration of the callback.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<i16>(), samples) };

                let written = callback_source.fill(out, frames);

                if written < frames {
                    out[written * channels as usize..].fill(0);
                    callback_counters
                        .frames_underrun
                        .fetch_add((frames - written) as u64, Ordering::Relaxed);
                }

                callback_counters
                    .frames_played
                    .fetch_add(frames as u64, Ordering::Relaxed);
                callback_counters.callbacks.fetch_add(1, Ordering::Relaxed);

                AudioCallbackResult::Continue
            }))
            .open_stream()
            .map_err(|error| PlaybackError::Platform(format!("open: {error}")))?;

        // Everything below is read back from the opened stream. None of it can
        // be inferred from what was requested.
        let frames_per_burst = stream.frames_per_burst().max(1) as u32;
        let granted = GrantedStream {
            frames_per_burst,
            exclusive: stream.sharing_mode() == AudioSharingMode::Exclusive,
            low_latency: stream.performance_mode() == AudioPerformanceMode::LowLatency,
            // A stream that got exclusive mode is on the MMAP path; shared
            // mode never is. AAudio exposes no direct query for this.
            mmap: stream.sharing_mode() == AudioSharingMode::Exclusive,
            format: Format {
                sample_rate: stream.sample_rate().max(0) as u32,
                bit_depth: format.bit_depth,
                channels: stream.channel_count().max(0) as u8,
                channel_mask: format.channel_mask,
            },
        };

        // Two bursts is the standard starting point: one being played, one
        // being filled. Anything smaller glitches on the first hiccup, and
        // anything larger is latency paid up front.
        let _ = stream.set_buffer_size_in_frames((frames_per_burst * 2) as i32);

        stream
            .request_start()
            .map_err(|error| PlaybackError::Platform(format!("start: {error}")))?;

        Ok(Self {
            stream,
            granted,
            counters,
        })
    }

    /// What the device actually granted.
    #[must_use]
    pub const fn granted(&self) -> GrantedStream {
        self.granted
    }

    /// Counters the callback maintains.
    #[must_use]
    pub fn counters(&self) -> Arc<PlaybackCounters> {
        Arc::clone(&self.counters)
    }

    /// Latency AAudio reports for the buffer as configured, in milliseconds.
    ///
    /// This is the buffer alone. It does not include the device path, which
    /// AAudio only exposes through timestamps and which no device reports
    /// honestly. See `docs/latency-budget.md`.
    #[must_use]
    pub fn buffer_latency_ms(&self) -> f64 {
        let frames = self.stream.buffer_size_in_frames().max(0) as f64;
        let rate = f64::from(self.granted.format.sample_rate.max(1));
        frames * 1000.0 / rate
    }

    /// Stop the stream and release the device.
    ///
    /// # Errors
    /// Returns [`PlaybackError::Platform`] if AAudio refuses to stop.
    pub fn stop(&self) -> Result<(), PlaybackError> {
        self.stream
            .request_stop()
            .map_err(|error| PlaybackError::Platform(format!("stop: {error}")))
    }
}

// SAFETY: AAudio's stream functions are documented as callable from any
// thread; the only threading rule is about what the data callback itself may
// call, and this type never calls anything from inside the callback. The
// restriction the ndk crate encodes is its own conservatism about the raw
// pointer, not a property of AAudio. Playback has to cross threads because the
// FFI object that owns it is shared with the JVM.
unsafe impl Send for Playback {}
// SAFETY: as above. Every method here either reads an immutable property of
// the stream or issues a control call, both of which AAudio serialises
// internally.
unsafe impl Sync for Playback {}

impl Drop for Playback {
    fn drop(&mut self) {
        // A stream left running holds the low-latency path open and keeps the
        // callback thread alive after the source is gone.
        let _ = self.stream.request_stop();
    }
}
