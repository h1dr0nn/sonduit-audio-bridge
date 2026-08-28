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

use crate::core::logging::{install_panic_hook, log_message};
use crate::core::window::apply_backdrop;
use tauri::Manager;

/// Build and run the Tauri application.
///
/// # Panics
/// Panics if the Tauri context cannot be built, which means the bundled
/// configuration or assets are missing and there is nothing to run.
pub fn run() {
    // Before anything else: there is no console in this build, so a panic
    // during setup would otherwise vanish without trace.
    install_panic_hook();

    tauri::Builder::default()
        .manage(bridge::BridgeState::default())
        // First, before any other plugin: a second copy must not get as far as
        // opening a window or binding a socket.
        //
        // Two copies of this app are not merely untidy. They contend for the
        // discovery port and for the capture endpoint, so the second one fails
        // in ways that look like a bug in the first -- and the user who
        // double-clicked the icon twice has no reason to connect the two. The
        // window that already exists is raised instead, which is what they
        // wanted from the second click anyway.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        // Remembers where the window was and how big it was.
        //
        // Not the maximised state: the window is undecorated and draws its own
        // titlebar, so restoring maximised is handled by the same code that
        // draws the buttons rather than by the plugin second-guessing it.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::SIZE,
                )
                .build(),
        )
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
            commands::bridge_invite,
            commands::bridge_await_pairing,
            commands::bridge_cancel_pairing,
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
