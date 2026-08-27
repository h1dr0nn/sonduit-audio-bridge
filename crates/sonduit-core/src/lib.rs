//! Latency-critical logic shared by every Sonduit platform.
//!
//! This crate owns packet encoding, the ring buffer, the adaptive jitter
//! buffer, drift estimation and telemetry. It is compiled into the Windows
//! sender and into the Android receiver, so the two can never disagree about
//! the wire format or about how a buffer is sized.
//!
//! # The structural rule
//!
//! **This crate has no platform dependencies and performs no I/O.** No
//! sockets, no audio APIs, no threads, no clocks. Time and arrival events are
//! passed in by the caller.
//!
//! That is not tidiness. It is what makes the jitter and drift logic testable
//! against a synthetic packet timeline on a machine with no audio hardware,
//! which is the only way this code can be trusted: bugs in an adaptive buffer
//! are close to undebuggable once they are behind a real network and a real
//! sound card. See `docs/environment.md` and CONTRIBUTING.md.
//!
//! Any pull request adding a platform dependency here should be rejected.

#![forbid(unsafe_code)]

pub mod drift;
pub mod format;
pub mod jitter;
pub mod metrics;
pub mod packet;
pub mod processor;
pub mod ring;

pub use format::{BitDepth, Format};
pub use metrics::Telemetry;

/// Anything that can go wrong decoding or encoding audio in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A buffer or datagram was not the size the format requires.
    #[error("expected {expected} bytes, got {actual}")]
    BadLength {
        /// Size the operation required.
        expected: usize,
        /// Size actually supplied.
        actual: usize,
    },

    /// The sample rate is not an integer multiple of 44100 or 48000, or the
    /// multiplier does not fit in the seven bits the wire format allows.
    #[error("sample rate {0} cannot be encoded")]
    BadSampleRate(u32),

    /// Sample width other than 16, 24 or 32 bits.
    #[error("unsupported bit depth {0}")]
    BadBitDepth(u8),

    /// Channel count outside 1 to 8.
    #[error("unsupported channel count {0}")]
    BadChannelCount(u8),

    /// The payload cannot be divided into whole frames for this format, so no
    /// receiver could split it correctly.
    #[error("payload does not divide into whole frames for this format")]
    UnrepresentableFormat,

    /// A datagram did not carry Sonduit's magic prefix.
    #[error("datagram is not a sonduit packet")]
    BadMagic,

    /// Wire format version this build does not understand.
    #[error("unsupported wire format version {0}")]
    UnsupportedVersion(u8),
}
