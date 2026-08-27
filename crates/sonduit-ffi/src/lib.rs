//! UniFFI surface exposing the shared core to the Android app.
//!
//! The Kotlin UI drives the bridge through this crate. The audio callback does
//! **not** cross this boundary: it stays entirely inside Rust and native code,
//! because calling into the JVM from a realtime callback can allocate, take VM
//! locks and stall on GC. See ADR-003.
//!
//! # Status
//!
//! Scaffolding only. The UniFFI interface definition and binding generation
//! are tracked in `docs/roadmap.md`.

#![forbid(unsafe_code)]

use sonduit_core::metrics::Telemetry;

/// Bridge lifecycle state, mirrored into the Android UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BridgeState {
    /// Not connected to a sender.
    #[default]
    Idle,
    /// Listening for a sender to announce itself.
    Discovering,
    /// Receiving audio.
    Streaming,
    /// Stopped because of an error.
    Failed,
}

/// Errors surfaced across the FFI boundary.
#[derive(Debug, thiserror::Error)]
pub enum FfiError {
    /// The bridge is not running.
    #[error("bridge is not running")]
    NotRunning,

    /// Transport failure.
    #[error("transport error: {0}")]
    Transport(String),
}

/// A handle the Android app holds for the lifetime of a session.
#[derive(Debug, Default)]
pub struct Bridge {
    state: BridgeState,
    telemetry: Telemetry,
}

impl Bridge {
    /// Create an idle bridge.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> BridgeState {
        self.state
    }

    /// Most recent telemetry snapshot.
    #[must_use]
    pub const fn telemetry(&self) -> Telemetry {
        self.telemetry
    }

    /// Begin listening for a sender.
    ///
    /// # Errors
    /// Returns [`FfiError::Transport`] when the socket cannot be bound.
    pub fn start(&mut self, _port: u16) -> Result<(), FfiError> {
        todo!("bind the receiver and start the audio stream: see docs/roadmap.md")
    }

    /// Stop and release the audio device.
    ///
    /// # Errors
    /// Returns [`FfiError::NotRunning`] when no session is active.
    pub fn stop(&mut self) -> Result<(), FfiError> {
        todo!("tear down the session: see docs/roadmap.md")
    }
}
