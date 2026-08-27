#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod core;

use crate::core::logging::log_message;
use crate::core::window::apply_backdrop;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
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
            commands::set_backdrop_theme
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {});
}
