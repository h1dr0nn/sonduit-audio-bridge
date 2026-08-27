//! The Sonduit desktop application.
//!
//! Exposed as a library as well as a binary so the audio path can be driven
//! from an integration test and from `examples/`. A bridge that can only be
//! started by clicking a button in a window is a bridge that is only ever
//! tested by hand, and this one has to run for hours without drifting.

pub mod bridge;
pub mod commands;
pub mod convert;
pub mod core;

use crate::core::logging::log_message;
use crate::core::window::apply_backdrop;
use tauri::Manager;

/// Build and run the Tauri application.
///
/// # Panics
/// Panics if the Tauri context cannot be built, which means the bundled
/// configuration or assets are missing and there is nothing to run.
pub fn run() {
    tauri::Builder::default()
        .manage(bridge::BridgeState::default())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                // Light is the startup default; the frontend retints through
                // set_backdrop_theme once it has read the stored preference.
                apply_backdrop(&window, false);
            }
            log_message("desktop", "sonduit desktop shell started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::set_backdrop_theme,
            commands::convert_audio,
            commands::analyze_audio,
            commands::bridge_snapshot,
            commands::bridge_scan,
            commands::bridge_start,
            commands::bridge_stop
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // A capture thread outliving the window would hold the audio
            // endpoint open and keep sending to a device nobody is watching.
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                let state = app.state::<bridge::BridgeState>();
                if state.is_running() {
                    log_message("desktop", "stopping the bridge before exit");
                    let _ = bridge::stop(&state);
                }
            }
        });
}
