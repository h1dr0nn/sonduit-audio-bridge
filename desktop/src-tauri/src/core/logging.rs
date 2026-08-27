//! Where the application says what happened.
//!
//! There is no console. `main.rs` builds with the Windows subsystem in every
//! configuration, because a GUI application that opens a black window beside
//! itself is broken however useful the window is to a developer. Everything
//! therefore has to reach a file, including the failures nobody was expecting.

use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;

/// The log file, beside the application's own data.
///
/// Deliberately not the temp directory: a user asked for the log after a crash
/// should be able to find it, and temp is cleaned by the system.
fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let directory = base.join("net.sonduit.app");
    std::fs::create_dir_all(&directory).ok()?;
    Some(directory.join("sonduit.log"))
}

/// Append one line, if a log file can be opened at all.
///
/// A failure to log is never propagated. Nothing this application does is
/// worth abandoning because the log could not be written.
fn append(line: &str) {
    let Some(path) = log_path() else { return };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

/// Record a message under a scope.
pub fn log_message(scope: &str, message: &str) {
    append(&format!(
        "[{}] [{}] {}",
        scope,
        Utc::now().to_rfc3339(),
        message
    ));
}

/// Send panics to the log file.
///
/// Without this a panic in a GUI build is a window that vanishes with no
/// explanation anywhere. The default hook writes to stderr, and there is no
/// stderr to write to.
///
/// The previous hook is kept and still called, so anything the Tauri or log
/// plugins install continues to work.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "unknown location".to_string(), ToString::to_string);

        // The payload is a &str for panic! and a String for format-argument
        // panics, and neither downcast covers the other.
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());

        log_message("panic", &format!("{message} at {location}"));
        previous(info);
    }));
}
