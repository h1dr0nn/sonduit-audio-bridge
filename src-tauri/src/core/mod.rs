//! Core utilities for the desktop shell.
//!
//! The desktop shell owns no audio logic. Everything latency-critical lives in
//! the `sonduit-core` crate so it can be shared with the Android target.

pub mod logging;
