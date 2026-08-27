//! IPC commands exposed to the frontend layer.

/// Liveness probe used by the frontend to confirm the Tauri backend is up.
#[tauri::command]
pub fn ping() -> String {
    "pong".to_string()
}
