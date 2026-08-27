//! Windows system audio capture.
//!
//! Only the platform-independent parts compile off Windows, so the workspace
//! still builds on the Linux CI runner that checks the shared core and the
//! Android targets.
//!
//! # Status
//!
//! The capture implementation is not written yet. See ADR-002: research
//! established that the prebuilt Scream driver binaries are unusable, which
//! changes what this crate has to do. `docs/roadmap.md` tracks the work.

#![forbid(unsafe_code)]

use sonduit_core::format::Format;

/// How system audio is being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// WASAPI loopback on a render endpoint.
    ///
    /// Taps an existing output device: audio still plays locally, and the
    /// stream stalls entirely while nothing is playing.
    EndpointLoopback,

    /// WASAPI process loopback.
    ///
    /// Endpoint-independent and emits silence rather than stalling, but needs
    /// Windows build 20348 or newer, which in retail terms means Windows 11.
    ProcessLoopback,
}

/// A capture endpoint Windows can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Stable device identifier.
    pub id: String,
    /// Name shown to the user.
    pub name: String,
    /// Whether Windows currently treats this as the default render device.
    pub is_default: bool,
}

/// Capture failures.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The requested mode is not available on this Windows build.
    #[error("capture mode {0:?} is unavailable on this system")]
    ModeUnavailable(CaptureMode),

    /// No render endpoint exists to capture from.
    #[error("no render endpoint available")]
    NoEndpoint,

    /// A Windows API call failed.
    #[error("windows api error: {0}")]
    Platform(String),
}

/// List the render endpoints that can be captured from.
///
/// # Errors
/// Returns [`CaptureError::Platform`] when device enumeration fails.
pub fn enumerate_endpoints() -> Result<Vec<Endpoint>, CaptureError> {
    #[cfg(windows)]
    {
        todo!("WASAPI endpoint enumeration: see docs/roadmap.md")
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::NoEndpoint)
    }
}

/// Open a capture stream.
///
/// # Errors
/// Returns [`CaptureError::ModeUnavailable`] when the Windows build is too old
/// for the requested mode.
pub fn open(_mode: CaptureMode, _format: Format) -> Result<(), CaptureError> {
    #[cfg(windows)]
    {
        todo!("WASAPI capture: see docs/roadmap.md")
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::ModeUnavailable(_mode))
    }
}
