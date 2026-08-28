//! Passing decoded audio from the receive thread to the audio callback.
//!
//! # What went wrong without this
//!
//! The two threads shared one mutex over the jitter buffer. The receive thread
//! took it on every packet, which at six milliseconds a packet is constantly,
//! and the audio callback took it with `try_lock` so it could never block.
//! That is realtime-safe and useless: the callback lost the race often, wrote
//! silence when it lost, and the buffer it was failing to drain grew to its
//! 1536 ms ceiling. On a real phone that was a second and a half of latency
//! with crackle through it, and the drift controller sat pinned at its
//! 500 ppm limit trying to resample away a second of audio, which would have
//! taken fifty minutes.
//!
//! # The two halves
//!
//! [`Producer`] lives on the receive thread and is fed whole packets.
//! [`Consumer`] lives in the audio callback, takes no lock and allocates
//! nothing. Neither ever waits for the other.
//!
//! # The emergency path
//!
//! Resampling corrects parts per million. It cannot correct a backlog, and
//! the roadmap has always said sample dropping is the emergency path. When the
//! queue is far deeper than the target the producer drops down to it in one
//! step: a single audible discontinuity now is better than latency that stays
//! for the rest of the session.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rtrb::{Consumer as RtrbConsumer, Producer as RtrbProducer, RingBuffer};

use crate::format::Format;

/// How much deeper than target the queue must get before audio is dropped.
///
/// Four times is far outside anything jitter produces and far inside the
/// point where the delay is obvious, so the drop happens once, early, rather
/// than repeatedly or too late to help.
const RESYNC_MULTIPLE: f64 = 4.0;

/// Create the two ends of the handoff.
///
/// `capacity_ms` bounds the queue, and therefore bounds the worst latency it
/// can hold. It must be comfortably above the jitter buffer's own target or
/// the producer will be dropping audio during ordinary operation.
#[must_use]
pub fn channel(format: Format, capacity_ms: u32) -> (Producer, Consumer) {
    let bytes_per_ms = format.sample_rate as usize * format.bytes_per_frame() / 1000;
    // At least one packet, so a degenerate format cannot produce a zero-length
    // queue that silently accepts nothing.
    let capacity = (bytes_per_ms * capacity_ms as usize).max(crate::format::PCM_PAYLOAD_BYTES);

    let (producer, consumer) = RingBuffer::new(capacity);

    // rtrb gives the read cursor to the consumer alone, and the consumer is
    // the audio callback. The receive thread therefore cannot drop the
    // backlog itself; it leaves a request here and the callback honours it on
    // its next pass, which is at most one burst away.
    let pending = Arc::new(AtomicUsize::new(0));

    (
        Producer {
            inner: producer,
            format,
            pending_skip: Arc::clone(&pending),
            dropped_frames: 0,
            resyncs: 0,
        },
        Consumer {
            inner: consumer,
            format,
            pending_skip: pending,
        },
    )
}

/// The receive thread's end.
pub struct Producer {
    inner: RtrbProducer<u8>,
    format: Format,
    /// Bytes the consumer has been asked to skip. See [`channel`].
    pending_skip: Arc<AtomicUsize>,
    dropped_frames: u64,
    resyncs: u64,
}

impl Producer {
    /// Push decoded PCM, returning how many bytes were taken.
    ///
    /// A short write means the callback has stalled. The caller is told rather
    /// than having the tail dropped for it, because losing audio is a decision
    /// that belongs where the context is.
    pub fn push(&mut self, pcm: &[u8]) -> usize {
        let mut written = 0;
        for byte in pcm {
            if self.inner.push(*byte).is_err() {
                break;
            }
            written += 1;
        }
        written
    }

    /// Bytes waiting to be played.
    ///
    /// `slots()` on the producer side counts free space, not occupancy, which
    /// is the opposite of what is wanted here and reads as though the queue
    /// were full the moment it is empty.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.inner.buffer().capacity() - self.inner.slots()
    }

    /// Milliseconds of audio waiting to be played.
    #[must_use]
    pub fn queued_ms(&self) -> f64 {
        self.bytes_to_ms(self.queued())
    }

    /// Drop back to `target_ms` if the queue has run far past it.
    ///
    /// Returns the frames discarded, which is zero in the ordinary case.
    /// Called by the receive thread, never by the callback: deciding to throw
    /// audio away is not something to do at realtime priority.
    pub fn resync_if_hopeless(&mut self, target_ms: f64) -> u64 {
        if target_ms <= 0.0 || self.queued_ms() < target_ms * RESYNC_MULTIPLE {
            return 0;
        }

        let keep = self.ms_to_bytes(target_ms);
        let excess = self.queued().saturating_sub(keep);
        // Whole frames only. Dropping a partial frame shifts every subsequent
        // sample into the wrong channel, which is far louder than the delay
        // being fixed.
        let frame = self.format.bytes_per_frame();
        let drop_bytes = excess / frame * frame;
        if drop_bytes == 0 {
            return 0;
        }

        let dropped = self.queued().min(drop_bytes);
        self.pending_skip.fetch_add(dropped, Ordering::Release);

        let frames = (dropped / frame) as u64;
        self.dropped_frames += frames;
        self.resyncs += 1;
        frames
    }

    /// Frames thrown away by resynchronisation over the session.
    #[must_use]
    pub const fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// How many times the queue has been resynchronised.
    ///
    /// More than a handful in a session means something upstream is wrong;
    /// this path exists to recover from a fault, not to run continuously.
    #[must_use]
    pub const fn resyncs(&self) -> u64 {
        self.resyncs
    }

    fn bytes_to_ms(&self, bytes: usize) -> f64 {
        let per_ms = self.format.sample_rate as f64 * self.format.bytes_per_frame() as f64 / 1000.0;
        if per_ms <= 0.0 {
            return 0.0;
        }
        bytes as f64 / per_ms
    }

    fn ms_to_bytes(&self, ms: f64) -> usize {
        let per_ms = self.format.sample_rate as f64 * self.format.bytes_per_frame() as f64 / 1000.0;
        (ms * per_ms) as usize
    }
}

/// The audio callback's end.
pub struct Consumer {
    inner: RtrbConsumer<u8>,
    format: Format,
    /// Bytes to throw away before the next fill. See [`channel`].
    pending_skip: Arc<AtomicUsize>,
}

impl Consumer {
    /// Fill `out` with interleaved samples for `frames` frames.
    ///
    /// Returns the frames written. Takes no lock and allocates nothing, which
    /// is the whole point: this runs at realtime priority and anything it
    /// waits on becomes a dropout.
    pub fn fill(&mut self, out: &mut [i16], frames: usize) -> usize {
        // Honour any resynchronisation the receive thread asked for. A chunk
        // commit moves the cursor without touching the bytes, so throwing away
        // a second of audio costs the same here as throwing away none.
        let skip = self.pending_skip.swap(0, Ordering::Acquire);
        if skip > 0 {
            self.skip(skip);
        }

        let channels = self.format.channels as usize;
        let wanted = (frames * channels).min(out.len());
        let mut written = 0;

        while written < wanted {
            let Ok(low) = self.inner.pop() else { break };
            let Ok(high) = self.inner.pop() else {
                // A byte short of a whole sample. The queue is byte-addressed
                // and the odd byte is unusable, so it goes rather than being
                // paired with whatever arrives next.
                break;
            };
            out[written] = i16::from_le_bytes([low, high]);
            written += 1;
        }

        written / channels
    }

    /// Bytes waiting.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.inner.slots()
    }

    /// Throw away up to `bytes` from the front.
    ///
    /// Public so a test can drive it directly. In the running application it
    /// is called by `fill` on the request the producer left behind.
    pub fn skip(&mut self, bytes: usize) -> usize {
        let take = bytes.min(self.inner.slots());
        self.inner.read_chunk(take).map_or(0, |chunk| {
            chunk.commit_all();
            take
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(frames: usize, value: i16) -> Vec<u8> {
        let mut out = Vec::with_capacity(frames * 4);
        for _ in 0..frames {
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    #[test]
    fn audio_comes_out_in_the_order_it_went_in() {
        let (mut producer, mut consumer) = channel(Format::stereo_48k(), 100);
        producer.push(&pcm(10, 1000));
        producer.push(&pcm(10, 2000));

        let mut out = [0_i16; 40];
        assert_eq!(consumer.fill(&mut out, 20), 20);
        assert_eq!(out[0], 1000);
        assert_eq!(out[20], 2000, "the second block did not follow the first");
    }

    #[test]
    fn an_empty_queue_yields_no_frames_rather_than_noise() {
        // The callback fills the rest with silence. Claiming frames that were
        // never written would leave the device playing whatever was in the
        // buffer.
        let (_producer, mut consumer) = channel(Format::stereo_48k(), 100);
        let mut out = [0_i16; 64];
        assert_eq!(consumer.fill(&mut out, 32), 0);
    }

    #[test]
    fn a_full_queue_reports_a_short_write_instead_of_dropping_the_tail() {
        let (mut producer, _consumer) = channel(Format::stereo_48k(), 10);
        let capacity = producer.queued() + producer.push(&pcm(48_000, 1));
        assert!(capacity > 0);
        assert_eq!(producer.push(&pcm(100, 1)), 0, "accepted into a full queue");
    }

    #[test]
    fn queued_time_matches_the_format() {
        // 48 kHz stereo 16-bit is 192 bytes per millisecond.
        let (mut producer, _consumer) = channel(Format::stereo_48k(), 100);
        producer.push(&pcm(480, 1)); // 10 ms
        assert!((producer.queued_ms() - 10.0).abs() < 0.01);
    }

    #[test]
    fn an_ordinary_depth_is_left_alone() {
        // Resynchronisation is an emergency. Firing it during normal jitter
        // would be a click every time the link hiccuped.
        let (mut producer, _consumer) = channel(Format::stereo_48k(), 500);
        producer.push(&pcm(48 * 40, 1)); // 40 ms against a 30 ms target
        assert_eq!(producer.resync_if_hopeless(30.0), 0);
        assert_eq!(producer.resyncs(), 0);
    }

    #[test]
    fn a_hopeless_backlog_is_dropped_to_target() {
        // The real case from the device: 1536 ms against a 30 ms target, which
        // resampling at its 500 ppm limit would need fifty minutes to clear.
        let (mut producer, mut consumer) = channel(Format::stereo_48k(), 2000);
        producer.push(&pcm(48 * 1536, 1));
        assert!(producer.queued_ms() > 1500.0);

        let dropped = producer.resync_if_hopeless(30.0);
        assert!(dropped > 0, "nothing was dropped");

        // The drop happens on the consumer's next pass, which is the callback.
        let mut out = [0_i16; 256];
        consumer.fill(&mut out, 64);

        assert!(
            consumer.queued() as f64 / 192.0 < 40.0,
            "still holding {} ms",
            consumer.queued() as f64 / 192.0
        );
        assert_eq!(producer.resyncs(), 1);
    }

    #[test]
    fn a_resync_reaches_the_consumer_without_the_caller_doing_anything() {
        // The request used to be recorded and never acted on, so the method
        // reported frames dropped while the backlog stayed exactly where it
        // was. That is the silent no-op CONTRIBUTING forbids.
        let (mut producer, mut consumer) = channel(Format::stereo_48k(), 2000);
        producer.push(&pcm(48 * 1000, 1));
        let before = consumer.queued();

        producer.resync_if_hopeless(30.0);

        let mut out = [0_i16; 64];
        consumer.fill(&mut out, 16);

        assert!(
            consumer.queued() < before / 2,
            "the backlog survived: {before} -> {}",
            consumer.queued()
        );
    }

    #[test]
    fn only_whole_frames_are_dropped() {
        // A partial frame shifts every later sample into the wrong channel,
        // which is far louder than the delay it was fixing.
        let (mut producer, _consumer) = channel(Format::stereo_48k(), 2000);
        producer.push(&pcm(48 * 1000, 1));

        let dropped = producer.resync_if_hopeless(30.0);
        assert!(dropped > 0);
        // Frames, so the byte count it stands for is a multiple of four.
        assert_eq!((dropped * 4) % 4, 0);
    }

    #[test]
    fn a_zero_target_never_triggers_a_drop() {
        // A caller that has not worked out its target yet must not be taken to
        // mean "hold nothing".
        let (mut producer, _consumer) = channel(Format::stereo_48k(), 500);
        producer.push(&pcm(48 * 400, 1));
        assert_eq!(producer.resync_if_hopeless(0.0), 0);
    }

    #[test]
    fn a_mono_stream_is_handled_as_well_as_a_stereo_one() {
        let format = Format {
            channels: 1,
            channel_mask: 0x0004,
            ..Format::stereo_48k()
        };
        let (mut producer, mut consumer) = channel(format, 100);

        let mut mono = Vec::new();
        for value in 0..10_i16 {
            mono.extend_from_slice(&value.to_le_bytes());
        }
        producer.push(&mono);

        let mut out = [0_i16; 10];
        assert_eq!(consumer.fill(&mut out, 10), 10);
        assert_eq!(out[3], 3);
    }
}
