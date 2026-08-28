//! IPC commands exposed to the frontend layer.

use crate::bridge::{
    self, AudioEndpoint, BridgeSnapshot, BridgeState, DiscoveredDevice, PairingInvite, SessionInfo,
    StartOptions,
};
use crate::convert::{self, BackendResult, ConvertPayload};
use crate::core::window::apply_backdrop;
use sonduit_transport::pairing::PairingCode;
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
pub async fn bridge_scan(
    state: State<'_, BridgeState>,
    code: String,
) -> Result<Vec<DiscoveredDevice>, String> {
    // Parsed here as well as inside the scan, because the credential has to
    // outlive the blocking task: it is what proves, later and over a different
    // path, that a phone answering on a cable is this phone. Without it a
    // session cannot migrate at all.
    let credential = PairingCode::parse(&code).ok_or("the pairing code must be six digits")?;

    let found = tauri::async_runtime::spawn_blocking(move || bridge::discover(&code))
        .await
        .map_err(|error| format!("background task failed: {error}"))?
        .map_err(String::from)?;

    // The key each device agreed stays behind, in the bridge's own list. What
    // crosses to the webview is the name and the address and nothing else:
    // `PairedDevice` holds the secret in a private field with no accessor, so
    // there is no way for one to be serialised into an IPC reply by accident.
    state.remember(&found, &credential);
    Ok(found
        .into_iter()
        .map(|paired| paired.device)
        .collect::<Vec<_>>())
}

/// Generate the pairing invite the connection page renders as a QR code.
///
/// Calling it again replaces the previous invite, so the code that was on
/// screen a moment ago stops being accepted.
#[tauri::command]
pub fn bridge_invite(state: State<'_, BridgeState>) -> Result<PairingInvite, String> {
    bridge::create_invite(&state).map_err(Into::into)
}

/// Wait for the phone that scanned the invite to announce itself.
///
/// Blocks for as long as the pairing window, so it runs off the async runtime.
/// Everything the wait needs is taken out of the state before it starts: a
/// Tauri `State` borrows, and the blocking task outlives the borrow.
///
/// `Ok(None)` means nobody scanned in time, or that the dialog was closed.
/// Neither is an error.
#[tauri::command]
pub async fn bridge_await_pairing(
    state: State<'_, BridgeState>,
) -> Result<Option<DiscoveredDevice>, String> {
    let session = state.pairing_session()?;
    let credential = session.code();

    let found = tauri::async_runtime::spawn_blocking(move || bridge::await_pairing(&session))
        .await
        .map_err(|error| format!("background task failed: {error}"))?
        .map_err(String::from)?;

    // The code that proved this device, kept for the same reason the scan
    // keeps its own: it is the only thing that can later prove the phone on a
    // newly appeared cable is the phone the session is streaming to. The
    // master secret agreed with it is kept in the same record and by the same
    // call, because both came out of this one pairing and neither outlives it.
    if let Some(paired) = &found {
        state.remember(std::slice::from_ref(paired), &credential);
    }
    Ok(found.map(|paired| paired.device))
}

/// Stop waiting, and retire the code that was on screen.
///
/// Called when the pairing dialog closes. Without it the wait would run for
/// the rest of its window against a dialog the user has dismissed, and the
/// code would still pair a device they are no longer expecting.
#[tauri::command]
pub fn bridge_cancel_pairing(state: State<'_, BridgeState>) {
    state.cancel_pairing();
}

/// The output devices the user can choose to capture from.
///
/// Enumeration walks the audio endpoints and reads a property store per
/// device, and it does it on a thread of its own for the apartment reasons in
/// [`bridge::endpoints`]. Joining that thread is a blocking wait, so the
/// command runs off the async runtime like the other blocking ones do.
#[tauri::command]
pub async fn bridge_endpoints() -> Result<Vec<AudioEndpoint>, String> {
    tauri::async_runtime::spawn_blocking(bridge::endpoints)
        .await
        .map_err(|error| format!("background task failed: {error}"))?
        .map_err(String::from)
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
