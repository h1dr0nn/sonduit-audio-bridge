//! Reserved seat for realtime DSP.
//!
//! **Nothing here is implemented.** The trait exists so that the boundary
//! between capture and transport is already a named place, and so that
//! [`crate::ring::RingBuffer::with_contiguous_mut`] has a stated reason to
//! exist. EQ, gain and compression land here later; see `docs/roadmap.md`.
//!
//! Defining the seat now is cheap. Retrofitting one through a shipped audio
//! path is not.

use crate::format::Format;

/// A realtime, in-place transform on a block of PCM.
///
/// # Realtime contract
///
/// [`AudioProcessor::process`] runs on the audio thread. Implementations must
/// not allocate, take a lock, perform I/O, or panic. They are handed a
/// mutable slice and are expected to work in place; returning a new buffer
/// would mean allocating, which is exactly what this contract forbids.
pub trait AudioProcessor: Send {
    /// Called when the stream format is known or changes.
    ///
    /// This is the only place an implementation may allocate. It runs off the
    /// audio thread.
    fn prepare(&mut self, format: Format);

    /// Transform one block of interleaved PCM in place.
    ///
    /// `pcm` is raw little-endian samples in the format last passed to
    /// [`AudioProcessor::prepare`], not decoded floats. Blocks are whatever
    /// contiguous run the ring buffer had available, so an implementation must
    /// not assume a fixed size or that a block starts on a frame boundary
    /// unless it has checked.
    fn process(&mut self, pcm: &mut [u8]);

    /// Drop any internal state, for a stream restart.
    fn reset(&mut self);
}
