//! Structured logging helpers for the Tauri layer.

use chrono::Utc;

pub fn log_message(scope: &str, message: &str) {
    println!("[{}] [{}] {}", scope, Utc::now().to_rfc3339(), message);
}
