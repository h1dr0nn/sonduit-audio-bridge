//! IPC commands exposed to the frontend layer.

use crate::core::window::apply_backdrop;
use tauri::WebviewWindow;

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
