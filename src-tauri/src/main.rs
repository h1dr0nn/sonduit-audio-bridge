#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod core;

use crate::core::logging::log_message;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .setup(|_app| {
            log_message("desktop", "sonduit desktop shell started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::ping])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {});
}
