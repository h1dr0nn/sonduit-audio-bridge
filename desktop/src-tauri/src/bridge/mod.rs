//! The live audio bridge: capture, packetize, send, report.
//!
//! # Why this lives in the desktop crate
//!
//! It is the only place that may depend on both Windows capture and the
//! transport. `sonduit-core` must stay free of platform and I/O so it can be
//! compiled for Android; `sonduit-capture-win` must not know about sockets.
//! Joining them is an application concern, and this is the application.
//!
//! # Threads
//!
//! One thread does capture and send. It must not block on anything else, so
//! the UI never reads its state directly: it publishes a [`Telemetry`]
//! snapshot behind a mutex, and a second thread samples that on a timer and
//! emits it to the webview. A dropped snapshot is a missed UI frame, which
//! does not matter; a blocked capture thread is a glitch, which does.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sonduit_core::format::Format;
use sonduit_transport::packetize::Packetizer;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::{discovery, TransportError, Wire, DEFAULT_PORT};
use tauri::{AppHandle, Emitter};

pub mod adapters;
mod telemetry;

pub use telemetry::{Accumulator, BridgeSnapshot, SessionInfo, TelemetryView};

/// Event name the webview subscribes to for telemetry.
pub const TELEMETRY_EVENT: &str = "sonduit://telemetry";

/// How often a snapshot is pushed to the webview.
///
/// Four a second reads as live without making the webview repaint constantly.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Engine period requested from WASAPI, in milliseconds.
///
/// `latency-budget.md` allots 10 ms to capture. Asking for less is possible but
/// raises the glitch rate on shared hardware for a saving the network then
/// swamps.
const CAPTURE_PERIOD_MS: u32 = 10;

/// How long a discovery scan listens for replies.
const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(1_500);

/// How long the capture loop pauses after a failed read.
const BACKOFF: Duration = Duration::from_millis(100);

/// Consecutive failed reads before the session is reported as broken.
///
/// One is normal when the default device changes underneath us. A second of
/// them is not.
const FAILURES_BEFORE_ERROR: u32 = 10;

/// A receiver that answered a discovery probe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredDevice {
    /// Address and port to send audio to.
    pub id: String,
    /// Name the device announced.
    pub name: String,
    /// Same as `id`, kept separate because the UI shows it as a subtitle.
    pub address: String,
}

/// What the user asked for when starting a session.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    /// Receiver address, as `host:port`. Absent means multicast.
    pub target: Option<String>,
    /// Local address to bind, which is what selects WiFi versus USB.
    pub bind: Option<String>,
    /// Send Scream-compatible datagrams instead of Sonduit ones.
    #[serde(default)]
    pub scream_compatible: bool,
}

/// Bridge failures, as the frontend sees them.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// A session is already running.
    #[error("a session is already running")]
    AlreadyRunning,

    /// Audio capture could not be opened.
    #[error("capture: {0}")]
    Capture(String),

    /// A socket operation failed.
    #[error("network: {0}")]
    Network(String),

    /// An address the user supplied could not be parsed.
    #[error("{0} is not a valid address")]
    BadAddress(String),

    /// The pairing code was not six digits.
    #[error("the pairing code must be six digits")]
    BadPairingCode,
}

impl From<BridgeError> for String {
    fn from(error: BridgeError) -> Self {
        error.to_string()
    }
}

impl From<TransportError> for BridgeError {
    fn from(error: TransportError) -> Self {
        Self::Network(error.to_string())
    }
}

/// A running session, owned by the Tauri state.
struct Running {
    stop: Arc<AtomicBool>,
    audio: Option<std::thread::JoinHandle<()>>,
    reporter: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    stopper: sonduit_capture_win::CaptureStopper,
}

/// Managed Tauri state for the bridge.
#[derive(Default)]
pub struct BridgeState {
    running: Mutex<Option<Running>>,
    snapshot: Arc<Mutex<BridgeSnapshot>>,
}

impl BridgeState {
    /// The most recent snapshot, for a UI that has just mounted and has not
    /// received an event yet.
    #[must_use]
    pub fn snapshot(&self) -> BridgeSnapshot {
        self.snapshot
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Whether a session is currently running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

/// Parse an address the user typed, defaulting the port.
///
/// A bare host is accepted because typing the port every time is friction, and
/// the port is not something a user should have to know.
fn parse_target(text: &str, default_port: u16) -> Result<SocketAddr, BridgeError> {
    let trimmed = text.trim();
    if let Ok(address) = trimmed.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    Err(BridgeError::BadAddress(trimmed.to_string()))
}

/// Multicast destination used when no device has been chosen.
fn default_target() -> SocketAddr {
    SocketAddr::from((sonduit_transport::DEFAULT_MULTICAST_GROUP, DEFAULT_PORT))
}

/// A nonce for one scan.
///
/// Freshness is what stops a captured announcement being replayed at the next
/// scan, so this must not be derived from anything an observer can predict
/// from a previous probe.
fn scan_nonce() -> [u8; NONCE_BYTES] {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let local = 0_u8;
    let address = std::ptr::addr_of!(local) as u64;

    let mut nonce = [0_u8; NONCE_BYTES];
    nonce[..8].copy_from_slice(&(now as u64).to_le_bytes());
    nonce[8..].copy_from_slice(&address.rotate_left(29).to_le_bytes());
    nonce
}

/// Broadcast a discovery probe and collect the replies that prove they know
/// the pairing code.
///
/// Replies that do not verify are dropped without comment. There is nothing
/// useful to tell the user about a device that answered and failed: it is
/// either a typo in the code or a device that should not be offered, and
/// naming it would make the second one look like the first.
///
/// # Errors
/// Returns [`BridgeError::BadPairingCode`] for a code that is not six digits,
/// and [`BridgeError::Network`] when the socket cannot be bound or the
/// broadcast is refused.
pub fn discover(code: &str) -> Result<Vec<DiscoveredDevice>, BridgeError> {
    let code = PairingCode::parse(code).ok_or(BridgeError::BadPairingCode)?;
    let nonce = scan_nonce();

    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .map_err(|error| BridgeError::Network(error.to_string()))?;
    socket
        .set_broadcast(true)
        .map_err(|error| BridgeError::Network(error.to_string()))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| BridgeError::Network(error.to_string()))?;

    let probe = discovery::encode_probe(&nonce);
    let broadcast = SocketAddr::from((Ipv4Addr::BROADCAST, discovery::DISCOVERY_PORT));
    // A single probe is enough on a wired link and often not enough on WiFi,
    // where the first broadcast after an idle period is regularly dropped.
    let mut reached_anything = false;
    for _ in 0..3 {
        if socket.send_to(&probe, broadcast).is_ok() {
            reached_anything = true;
        }
    }

    // A tethered phone is often not reachable by broadcast: the RNDIS
    // interface sits on the Public firewall profile and some drivers drop
    // 255.255.255.255 outright. It is reachable by unicast, and its address is
    // the gateway on that adapter, so ask it directly rather than hoping.
    for adapter in adapters::enumerate().unwrap_or_default() {
        if socket
            .send_to(&probe, adapter.target(discovery::DISCOVERY_PORT))
            .is_ok()
        {
            reached_anything = true;
        }
    }

    if !reached_anything {
        return Err(BridgeError::Network(
            "no interface accepted the discovery probe".to_string(),
        ));
    }

    let mut found: Vec<DiscoveredDevice> = Vec::new();
    let deadline = Instant::now() + DISCOVERY_TIMEOUT;
    let mut datagram = [0_u8; 256];

    while Instant::now() < deadline {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        let Some(announcement) = discovery::decode_announce(&datagram[..length], &nonce, &code)
        else {
            continue;
        };
        let address = discovery::audio_address(from, &announcement);
        let id = address.to_string();
        // The same device answers every probe, so replies are deduplicated by
        // address rather than shown three times.
        if found.iter().any(|device| device.id == id) {
            continue;
        }
        found.push(DiscoveredDevice {
            name: announcement.name.clone(),
            address: id.clone(),
            id,
        });
    }

    Ok(found)
}

/// Start capturing and sending.
///
/// # Errors
/// Returns [`BridgeError::AlreadyRunning`] if a session is live, and reports
/// capture or socket failures otherwise.
#[cfg(windows)]
pub fn start(
    app: &AppHandle,
    state: &BridgeState,
    options: StartOptions,
) -> Result<SessionInfo, BridgeError> {
    use sonduit_capture_win::{open, CaptureMode};

    let mut guard = state
        .running
        .lock()
        .map_err(|_| BridgeError::Capture("bridge state is poisoned".into()))?;
    if guard.is_some() {
        return Err(BridgeError::AlreadyRunning);
    }

    let target = match options.target.as_deref() {
        Some(text) => parse_target(text, DEFAULT_PORT)?,
        None => default_target(),
    };
    let bind = match options.bind.as_deref() {
        Some(text) => parse_target(text, 0)?,
        None => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
    };

    let socket = UdpSocket::bind(bind).map_err(|error| BridgeError::Network(error.to_string()))?;
    if target.ip().is_multicast() {
        socket
            .set_multicast_ttl_v4(1)
            .map_err(|error| BridgeError::Network(error.to_string()))?;
    } else if target.ip().is_ipv4() && matches!(target.ip(), IpAddr::V4(ip) if ip.is_broadcast()) {
        socket
            .set_broadcast(true)
            .map_err(|error| BridgeError::Network(error.to_string()))?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::clone(&state.snapshot);
    let wire = if options.scream_compatible {
        Wire::Scream
    } else {
        Wire::Sonduit
    };

    // Capture is opened on the thread that will use it, not here. WASAPI is
    // COM: the interfaces are apartment-bound and the apartment is per-thread,
    // so a client opened here and moved would be used from a thread that never
    // initialised COM. The channel keeps the error reporting synchronous
    // anyway, so a failure still lands on the button the user pressed rather
    // than as a session that starts and then silently dies.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let audio = {
        let stop = Arc::clone(&stop);
        let snapshot = Arc::clone(&snapshot);
        std::thread::Builder::new()
            .name("sonduit-capture".into())
            .spawn(move || {
                let mut capture = match open(CaptureMode::EndpointLoopback, CAPTURE_PERIOD_MS) {
                    Ok(capture) => capture,
                    Err(error) => {
                        let _ = ready_tx.send(Err(match error {
                            sonduit_capture_win::CaptureError::NoEndpoint => {
                                BridgeError::Capture("no playback device to capture from".into())
                            }
                            other => BridgeError::Capture(other.to_string()),
                        }));
                        return;
                    }
                };

                let format = capture.format();
                let endpoint = sonduit_capture_win::enumerate_endpoints()
                    .ok()
                    .and_then(|endpoints| {
                        endpoints
                            .into_iter()
                            .find(|endpoint| endpoint.is_default)
                            .map(|endpoint| endpoint.name)
                    })
                    .unwrap_or_else(|| "Default output".to_string());

                if ready_tx
                    .send(Ok((format, endpoint, capture.stopper())))
                    .is_err()
                {
                    // The caller gave up waiting, so there is nobody to send
                    // audio for.
                    return;
                }

                capture_to_socket(
                    &mut capture,
                    &socket,
                    target,
                    format,
                    wire,
                    &stop,
                    &snapshot,
                );
            })
            .map_err(|error| BridgeError::Capture(error.to_string()))?
    };

    let (format, endpoint, stopper) = match ready_rx.recv() {
        Ok(Ok(ready)) => ready,
        Ok(Err(error)) => {
            let _ = audio.join();
            return Err(error);
        }
        Err(_) => {
            let _ = audio.join();
            return Err(BridgeError::Capture(
                "the capture thread stopped before it reported a format".into(),
            ));
        }
    };

    let info = SessionInfo::new(&endpoint, format, target, options.scream_compatible);

    {
        let mut current = snapshot
            .lock()
            .map_err(|_| BridgeError::Capture("telemetry state is poisoned".into()))?;
        *current = BridgeSnapshot::starting(info.clone());
    }

    let reporter = {
        let stop = Arc::clone(&stop);
        let snapshot = Arc::clone(&snapshot);
        let app = app.clone();
        std::thread::Builder::new()
            .name("sonduit-telemetry".into())
            .spawn(move || report(&app, &stop, &snapshot))
            .map_err(|error| BridgeError::Capture(error.to_string()))?
    };

    *guard = Some(Running {
        stop,
        audio: Some(audio),
        reporter: Some(reporter),
        stopper,
    });

    Ok(info)
}

/// Off Windows there is nothing to capture.
///
/// # Errors
/// Always returns [`BridgeError::Capture`].
#[cfg(not(windows))]
pub fn start(
    _app: &AppHandle,
    _state: &BridgeState,
    _options: StartOptions,
) -> Result<SessionInfo, BridgeError> {
    Err(BridgeError::Capture(
        "system audio capture is implemented for Windows only".into(),
    ))
}

/// The capture and send loop.
///
/// Public so an example can drive it without a window: a bridge that can only
/// be started by clicking a button is a bridge that is only ever tested by
/// hand. Returns when `stop` is set.
///
/// Errors are counted rather than propagated: a session that stops on the first
/// dropped datagram would be useless on WiFi, where transient send failures are
/// normal when an interface reconfigures.
#[cfg(windows)]
pub fn capture_to_socket(
    capture: &mut sonduit_capture_win::LoopbackCapture,
    socket: &UdpSocket,
    target: SocketAddr,
    format: Format,
    wire: Wire,
    stop: &AtomicBool,
    snapshot: &Mutex<BridgeSnapshot>,
) {
    let mut packetizer = Packetizer::new(format, wire);
    let mut pcm = Vec::with_capacity(1 << 16);
    let mut counters = telemetry::Accumulator::new(format);
    let mut consecutive_failures = 0_u32;

    while !stop.load(Ordering::Relaxed) {
        pcm.clear();
        let frames = match capture.read(&mut pcm) {
            Ok(frames) => {
                consecutive_failures = 0;
                frames
            }
            Err(error) => {
                counters.record_capture_error(&error.to_string());
                consecutive_failures += 1;
                if consecutive_failures == FAILURES_BEFORE_ERROR {
                    // Only now is this worth showing as a broken session. A
                    // single failed read happens when the default device
                    // changes and recovers by itself; a second of them does
                    // not.
                    if let Ok(mut current) = snapshot.lock() {
                        current.mark_error(&error.to_string());
                    }
                }
                // The endpoint has probably gone: default device changed,
                // headphones unplugged. Backing off keeps the loop from
                // spinning at full speed while the user works out what broke.
                std::thread::sleep(BACKOFF);
                continue;
            }
        };

        if frames > 0 {
            let result = packetizer.push(&pcm, |datagram| {
                socket
                    .send_to(datagram, target)
                    .map(|_| ())
                    .map_err(TransportError::from)
            });
            match result {
                Ok(()) => counters.record_sent(frames, packetizer.packets()),
                Err(error) => counters.record_send_error(&error.to_string()),
            }
        }

        if let Some(view) = counters.due() {
            if let Ok(mut current) = snapshot.lock() {
                current.apply(view);
                // The last failure travels with the readings so the user can
                // see why loss is non-zero, but it does not change the status:
                // a link dropping the occasional datagram is still working.
                current.note_error(counters.last_error());
            }
        }
    }

    if let Ok(mut current) = snapshot.lock() {
        // A last view before clearing, so a session that sent nothing at all
        // is distinguishable in the log from one that was simply stopped.
        current.apply(counters.view_now());
        current.mark_stopped();
    }
}

/// Push snapshots to the webview on a timer.
fn report(app: &AppHandle, stop: &AtomicBool, snapshot: &Mutex<BridgeSnapshot>) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(TELEMETRY_INTERVAL);
        let current = match snapshot.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => break,
        };
        // A failed emit means the window has gone. That is not an error worth
        // logging every 250 ms.
        let _ = app.emit(TELEMETRY_EVENT, &current);
    }

    if let Ok(guard) = snapshot.lock() {
        let _ = app.emit(TELEMETRY_EVENT, &*guard);
    }
}

/// Stop the running session and wait for its threads.
///
/// Idempotent: stopping a stopped bridge is not an error, because the UI can
/// send it on window close as well as on the button.
///
/// # Errors
/// Returns [`BridgeError::Capture`] if the state mutex is poisoned.
pub fn stop(state: &BridgeState) -> Result<(), BridgeError> {
    let mut guard = state
        .running
        .lock()
        .map_err(|_| BridgeError::Capture("bridge state is poisoned".into()))?;

    let Some(mut running) = guard.take() else {
        return Ok(());
    };

    running.stop.store(true, Ordering::Relaxed);
    // The capture thread is blocked in WaitForSingleObject, which the flag
    // alone will not break; the stopper signals the event so it wakes now
    // rather than after the two-second timeout.
    #[cfg(windows)]
    running.stopper.stop();

    if let Some(handle) = running.audio.take() {
        let _ = handle.join();
    }
    if let Some(handle) = running.reporter.take() {
        let _ = handle.join();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_address_gets_the_default_port() {
        // Users should not have to know the port number.
        let address = parse_target("192.168.1.50", DEFAULT_PORT).unwrap();
        assert_eq!(address, SocketAddr::from(([192, 168, 1, 50], DEFAULT_PORT)));
    }

    #[test]
    fn an_explicit_port_wins_over_the_default() {
        let address = parse_target("192.168.1.50:9999", DEFAULT_PORT).unwrap();
        assert_eq!(address.port(), 9999);
    }

    #[test]
    fn surrounding_whitespace_is_forgiven() {
        // Addresses arrive pasted, and a trailing space is not a typo worth
        // refusing.
        assert!(parse_target("  10.0.0.2  ", DEFAULT_PORT).is_ok());
    }

    #[test]
    fn a_hostname_is_refused_rather_than_silently_resolved() {
        // Resolution would block the command thread on DNS, and over USB
        // tethering there is no resolver to answer.
        let error = parse_target("my-phone.local", DEFAULT_PORT).unwrap_err();
        assert!(matches!(error, BridgeError::BadAddress(_)));
    }

    #[test]
    fn an_empty_address_is_refused() {
        assert!(parse_target("", DEFAULT_PORT).is_err());
    }

    #[test]
    fn the_default_target_is_the_scream_multicast_group() {
        // Inherited so an unmodified Scream receiver can be tested against.
        let target = default_target();
        assert!(target.ip().is_multicast());
        assert_eq!(target.port(), DEFAULT_PORT);
    }

    #[test]
    fn a_fresh_state_is_not_running_and_reports_an_empty_snapshot() {
        let state = BridgeState::default();
        assert!(!state.is_running());
        assert_eq!(state.snapshot().status, "disconnected");
    }

    #[test]
    fn stopping_a_stopped_bridge_is_not_an_error() {
        // The window close handler calls this unconditionally.
        let state = BridgeState::default();
        assert!(stop(&state).is_ok());
        assert!(stop(&state).is_ok());
    }
}
