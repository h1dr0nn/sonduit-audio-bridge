//! Android audio playback.
//!
//! Only the platform-independent parts compile off Android, so the workspace
//! builds on any host.
//!
//! # Status
//!
//! The AAudio implementation is not written yet; see `docs/roadmap.md`.
//! ADR-003 records why this targets `ndk::audio` rather than the `oboe` crate.

#![forbid(unsafe_code)]

use sonduit_core::format::Format;

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

/// Open an output stream, requesting exclusive low-latency mode.
///
/// # Errors
/// Returns [`PlaybackError::Unsupported`] off Android.
pub fn open(_format: Format) -> Result<GrantedStream, PlaybackError> {
    #[cfg(target_os = "android")]
    {
        todo!("AAudio stream open via ndk::audio: see docs/roadmap.md")
    }
    #[cfg(not(target_os = "android"))]
    {
        Err(PlaybackError::Unsupported)
    }
}
