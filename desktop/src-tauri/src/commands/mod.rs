//! IPC commands exposed to the frontend layer.

use crate::convert::{self, BackendResult, ConvertPayload};
use crate::core::window::apply_backdrop;
use tauri::{AppHandle, WebviewWindow};

/// Liveness probe used by the frontend to confirm the Tauri backend is up.
#[tauri::command]
pub fn ping() -> String {
    "pong".to_string()
}

/// Retint the native acrylic backdrop after the user switches theme.
///
/// The theme lives in the webview (local storage), so the frontend is the only
/// side that knows which tint applies; it calls this on mount and on toggle.
#[tauri::command]
pub fn set_backdrop_theme(window: WebviewWindow, dark: bool) {
    apply_backdrop(&window, dark);
}

/// Run an editor job: convert, master, trim or modify.
///
/// FFmpeg is a blocking child process, so the work moves off the async runtime
/// rather than stalling every other command for the length of a batch.
#[tauri::command]
pub async fn convert_audio(
    app: AppHandle,
    payload: ConvertPayload,
) -> Result<BackendResult, String> {
    tauri::async_runtime::spawn_blocking(move || convert::run(&app, payload))
        .await
        .map_err(|error| format!("background task failed: {error}"))
}

/// Measure a file and suggest a mastering preset.
#[tauri::command]
pub async fn analyze_audio(
    app: AppHandle,
    payload: ConvertPayload,
) -> Result<BackendResult, String> {
    tauri::async_runtime::spawn_blocking(move || convert::analyze(&app, payload))
        .await
        .map_err(|error| format!("background task failed: {error}"))
}
