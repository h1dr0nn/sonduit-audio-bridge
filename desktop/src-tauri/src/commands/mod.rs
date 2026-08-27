//! IPC commands exposed to the frontend layer.

use crate::bridge::{
    self, BridgeSnapshot, BridgeState, DiscoveredDevice, SessionInfo, StartOptions,
};
use crate::convert::{self, BackendResult, ConvertPayload};
use crate::core::window::apply_backdrop;
use tauri::{AppHandle, State, WebviewWindow};

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

/// The bridge state as it stands right now.
///
/// The UI subscribes to the telemetry event, but a window that has just
/// mounted has not received one yet and would otherwise render an empty shell
/// over a running session.
#[tauri::command]
pub fn bridge_snapshot(state: State<'_, BridgeState>) -> BridgeSnapshot {
    state.snapshot()
}

/// Broadcast a discovery probe and return the devices that answered with a
/// tag proving they know `code`.
///
/// Blocking for as long as the scan window, so it runs off the async runtime.
#[tauri::command]
pub async fn bridge_scan(code: String) -> Result<Vec<DiscoveredDevice>, String> {
    tauri::async_runtime::spawn_blocking(move || bridge::discover(&code))
        .await
        .map_err(|error| format!("background task failed: {error}"))?
        .map_err(Into::into)
}

/// Start capturing system audio and sending it.
#[tauri::command]
pub fn bridge_start(
    app: AppHandle,
    state: State<'_, BridgeState>,
    options: StartOptions,
) -> Result<SessionInfo, String> {
    bridge::start(&app, &state, options).map_err(Into::into)
}

/// Stop the running session. Not an error if none is running.
#[tauri::command]
pub fn bridge_stop(state: State<'_, BridgeState>) -> Result<(), String> {
    bridge::stop(&state).map_err(Into::into)
}
