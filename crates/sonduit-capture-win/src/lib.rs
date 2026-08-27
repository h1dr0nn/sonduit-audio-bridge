//! Windows system audio capture.
//!
//! Only the platform-independent parts compile off Windows, so the workspace
//! still builds on the Linux CI runner that checks the shared core and the
//! Android targets.
//!
//! # Why this crate contains `unsafe`
//!
//! WASAPI is a COM API. Every call is `unsafe` in the `windows` crate because
//! the compiler cannot check COM's own rules: that `CoInitializeEx` ran on this
//! thread, that a buffer from `GetBuffer` is released exactly once, that a
//! format pointer is freed with `CoTaskMemFree`. The safety comments in
//! [`wasapi`] record which rule each block is relying on. Nothing outside that
//! module is `unsafe`, and no `unsafe` crosses this crate's public API.

use sonduit_core::format::Format;

#[cfg(windows)]
pub mod wasapi;

#[cfg(windows)]
pub use wasapi::{CaptureStopper, LoopbackCapture};

/// How system audio is being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// WASAPI loopback on a render endpoint.
    ///
    /// Taps an existing output device: audio still plays locally, and without
    /// the silent render stream described in [`wasapi`] the stream stalls
    /// entirely while nothing is playing.
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
/// Returns [`CaptureError::Platform`] when device enumeration fails, and
/// [`CaptureError::NoEndpoint`] when the machine has no active render device.
pub fn enumerate_endpoints() -> Result<Vec<Endpoint>, CaptureError> {
    #[cfg(windows)]
    {
        wasapi::enumerate()
    }
    #[cfg(not(windows))]
    {
        Err(CaptureError::NoEndpoint)
    }
}

/// Open a loopback capture stream on the default render endpoint.
///
/// `period_ms` is the engine period requested through the silent render stream.
/// The returned stream reports the format it will actually deliver, which comes
/// from the engine and is not negotiable in shared mode; `requested` is only
/// used to warn when the two disagree.
///
/// # Errors
/// Returns [`CaptureError::ModeUnavailable`] for [`CaptureMode::ProcessLoopback`],
/// which is not implemented yet, and [`CaptureError::Platform`] when a WASAPI
/// call fails.
#[cfg(windows)]
pub fn open(mode: CaptureMode, period_ms: u32) -> Result<LoopbackCapture, CaptureError> {
    match mode {
        CaptureMode::EndpointLoopback => LoopbackCapture::open(period_ms),
        // Process loopback needs ActivateAudioInterfaceAsync with a completion
        // handler, which is a different activation path rather than a flag.
        // Tracked in docs/roadmap.md.
        CaptureMode::ProcessLoopback => Err(CaptureError::ModeUnavailable(mode)),
    }
}

/// Off Windows there is nothing to open.
///
/// # Errors
/// Always returns [`CaptureError::ModeUnavailable`].
#[cfg(not(windows))]
pub fn open(mode: CaptureMode, _period_ms: u32) -> Result<(), CaptureError> {
    Err(CaptureError::ModeUnavailable(mode))
}

/// The format Sonduit puts on the wire, given what the engine is mixing.
///
/// Kept out of the platform module so it can be tested on any host.
#[must_use]
pub fn wire_format(engine_sample_rate: u32, engine_channels: u8) -> Format {
    use sonduit_core::format::BitDepth;

    Format {
        sample_rate: engine_sample_rate,
        // The engine mixes 32-bit float; the wire carries 16-bit integer.
        bit_depth: BitDepth::S16,
        channels: engine_channels,
        channel_mask: if engine_channels == 1 { 0x0004 } else { 0x0003 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_keeps_the_engine_rate_and_channel_count() {
        // Resampling on the capture side would add latency and a failure mode
        // for no benefit: the engine rate is what the user is listening to.
        let format = wire_format(48_000, 2);

        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.channels, 2);
        assert!(format.validate().is_ok());
    }

    #[test]
    fn a_mono_engine_is_described_with_the_centre_channel() {
        let format = wire_format(44_100, 1);

        assert_eq!(format.channel_mask, 0x0004);
        assert!(format.validate().is_ok());
    }

    #[cfg(not(windows))]
    #[test]
    fn opening_off_windows_reports_the_mode_as_unavailable() {
        let error = open(CaptureMode::EndpointLoopback, 10).unwrap_err();
        assert!(matches!(error, CaptureError::ModeUnavailable(_)));
    }
}
