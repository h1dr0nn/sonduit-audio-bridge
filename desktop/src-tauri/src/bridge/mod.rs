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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sonduit_core::format::Format;
use sonduit_transport::feedback::{Feedback, FEEDBACK_BYTES};
use sonduit_transport::invite::Invite;
use sonduit_transport::packetize::Packetizer;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::roundtrip::RoundTrip;
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

/// How long the desktop waits for the phone after a QR invite goes on screen.
///
/// Generous, because the user has to pick up the phone, open the app and line
/// up a camera, and a timeout that expires while they are still doing that
/// reads as the feature being broken. Bounded all the same: the socket holds
/// the discovery port while it waits.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the capture loop pauses after a failed read.
const BACKOFF: Duration = Duration::from_millis(100);

/// Consecutive failed reads before the session is reported as broken.
///
/// One is normal when the default device changes underneath us. A second of
/// them is not.
const FAILURES_BEFORE_ERROR: u32 = 10;

/// Consecutive failed reads before the capture device is reopened.
///
/// Lower than the error threshold on purpose: recovery is attempted before the
/// user is told anything is wrong, so an unplugged headset usually looks like
/// nothing more than a short gap.
const FAILURES_BEFORE_REOPEN: u32 = 3;

/// How long a reopened session waits before trying again after a failure.
///
/// Reopening is far more expensive than a failed read, and a device that is
/// genuinely gone will refuse every attempt. This keeps a disconnected session
/// from hammering the audio engine.
const REOPEN_BACKOFF: Duration = Duration::from_millis(750);

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

/// A pairing invite, as the QR panel needs it.
///
/// The pairing code is deliberately not in here. It is inside `payload`
/// because the phone needs it, and putting it in a second field would invite a
/// UI that prints it somewhere the payload is not, which is a secret on screen
/// for longer than it has to be.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInvite {
    /// The text to render as a QR code.
    pub payload: String,
    /// The addresses the phone was offered, so the user can see which links
    /// this invite covers.
    pub addresses: Vec<String>,
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

    /// This machine has no address a phone could send to.
    #[error("this computer has no network address a phone could reach")]
    NoLocalAddress,

    /// Pairing was asked to wait with no invite on screen.
    #[error("no pairing code is on screen")]
    NoInvite,
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
    /// The invite currently on screen.
    ///
    /// Held because the announcement the phone sends back is authenticated
    /// against the code and nonce that particular QR carried, and a new invite
    /// must invalidate the old one rather than adding a second key that works.
    invite: Mutex<Option<Invite>>,
    /// The discovery-port listener, bound once and then reused.
    ///
    /// Binding a fresh socket per invite meant a second invite inside the
    /// pairing window failed with "only one usage of each socket address": the
    /// first wait still owned the port for the rest of its ninety seconds,
    /// whether or not the user was still looking at the code. Nothing releases
    /// a socket early on the strength of the caller having lost interest, so
    /// the socket outlives the invites instead.
    ///
    /// Datagrams left over from an earlier window are harmless. Every one is
    /// authenticated against the invite in hand, so an announcement keyed by a
    /// code that has been replaced fails to verify like any other stranger.
    listener: Mutex<Option<Arc<UdpSocket>>>,
    /// Bumped whenever a wait is superseded or cancelled.
    ///
    /// A wait watches this and gives up as soon as it changes, so closing the
    /// dialog stops the listen instead of leaving it running behind a window
    /// nobody can see.
    pairing_epoch: Arc<AtomicU64>,
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

    /// The invite currently on screen, if one is.
    #[must_use]
    pub fn invite(&self) -> Option<Invite> {
        self.invite.lock().ok().and_then(|guard| guard.clone())
    }

    /// Everything a wait needs, owned, plus the epoch it belongs to.
    ///
    /// Owned because the wait runs on a blocking task that outlives the
    /// borrow a Tauri `State` gives out.
    ///
    /// # Errors
    /// Returns [`BridgeError::NoInvite`] when no code is on screen, and
    /// [`BridgeError::Network`] when the discovery port cannot be bound, which
    /// on Windows is most often the firewall or a second copy of the app.
    pub fn pairing_session(&self) -> Result<PairingSession, BridgeError> {
        let invite = self.invite().ok_or(BridgeError::NoInvite)?;

        let mut slot = self
            .listener
            .lock()
            .map_err(|_| BridgeError::Network("pairing listener is poisoned".into()))?;
        let socket = match slot.as_ref() {
            Some(socket) => Arc::clone(socket),
            None => {
                let socket =
                    UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, invite.port)))
                        .map_err(|error| BridgeError::Network(error.to_string()))?;
                // Short enough that cancelling is felt as immediate, long
                // enough that waiting ninety seconds is not a spin.
                socket
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .map_err(|error| BridgeError::Network(error.to_string()))?;
                let socket = Arc::new(socket);
                *slot = Some(Arc::clone(&socket));
                socket
            }
        };

        // Claiming an epoch is what supersedes the previous wait; two waits on
        // one socket would otherwise race for the same datagram.
        let epoch = self.pairing_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        Ok(PairingSession {
            invite,
            socket,
            epoch,
            generation: Arc::clone(&self.pairing_epoch),
        })
    }

    /// Stop whatever wait is in progress.
    ///
    /// Called when the user closes the pairing dialog. The code stops being
    /// accepted at the same time, so a window that is no longer on screen
    /// cannot pair a device the user is no longer expecting.
    pub fn cancel_pairing(&self) {
        self.pairing_epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut current) = self.invite.lock() {
            *current = None;
        }
    }
}

/// One attempt to catch the phone that scanned the code on screen.
pub struct PairingSession {
    invite: Invite,
    socket: Arc<UdpSocket>,
    /// The epoch this wait claimed. It runs until `generation` moves past it.
    epoch: u64,
    generation: Arc<AtomicU64>,
}

impl PairingSession {
    /// Whether a newer invite, or a cancellation, has superseded this wait.
    fn superseded(&self) -> bool {
        self.generation.load(Ordering::SeqCst) != self.epoch
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

/// A pairing code seed, taken from the first eight bytes of a fresh nonce.
///
/// Shifting rather than `from_le_bytes` on a slice keeps this free of a
/// fallible conversion the caller would have to unwrap, in a function whose
/// only job is to move bits.
fn seed_from(nonce: &[u8; NONCE_BYTES]) -> u64 {
    let mut seed = 0_u64;
    for byte in nonce.iter().take(8) {
        seed = (seed << 8) | u64::from(*byte);
    }
    seed
}

/// Build the invite the QR panel shows, replacing any previous one.
///
/// The code is generated here rather than on the phone because the desktop is
/// the side displaying it. That inverts who has to read what: instead of the
/// user copying six digits off the phone and the phone's address after them,
/// the phone's camera reads both, and the desktop learns the phone's address
/// from the datagram that comes back.
///
/// # Errors
/// Returns [`BridgeError::Network`] when the adapter list cannot be read, and
/// [`BridgeError::NoLocalAddress`] when nothing on it is an address a phone
/// could send to.
pub fn create_invite(state: &BridgeState) -> Result<PairingInvite, BridgeError> {
    let addresses = adapters::local_ipv4().map_err(BridgeError::Network)?;

    let nonce = scan_nonce();
    let code = PairingCode::from_seed(seed_from(&nonce));

    let invite = Invite::new(&addresses, discovery::DISCOVERY_PORT, code, nonce)
        .ok_or(BridgeError::NoLocalAddress)?;

    let view = PairingInvite {
        payload: invite.to_payload(),
        addresses: invite
            .addresses
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
    };

    if let Ok(mut current) = state.invite.lock() {
        // Replacing rather than adding: the previous code must stop working
        // the moment a new one is on screen, or every invite ever shown would
        // stay valid for the life of the process.
        *current = Some(invite);
    }

    Ok(view)
}

/// Wait for an announcement from the phone that scanned the invite.
///
/// The phone sends this by unicast to an address the QR gave it, which is why
/// this works across subnets where the broadcast probe never arrives. The
/// phone's own address is taken from the datagram's source, never from
/// anything inside it: an address in a payload is an address the sender chose.
///
/// Returns `Ok(None)` when nothing verified within [`PAIRING_TIMEOUT`], which
/// is not an error. It usually means the user has not scanned yet.
///
/// Returns `Ok(None)` just as readily when the dialog is closed, which is why
/// the socket is not bound here: see [`BridgeState::pairing_session`].
///
/// # Errors
/// Infallible today. The signature keeps the `Result` because the caller is a
/// command and every other one of these can fail.
pub fn await_pairing(session: &PairingSession) -> Result<Option<DiscoveredDevice>, BridgeError> {
    let invite = &session.invite;
    let deadline = Instant::now() + PAIRING_TIMEOUT;
    let mut datagram = [0_u8; 256];

    while Instant::now() < deadline && !session.superseded() {
        let Ok((length, from)) = session.socket.recv_from(&mut datagram) else {
            continue;
        };
        // Anything that does not verify is dropped in silence, exactly as in
        // a broadcast scan: it is either a stray datagram on a port Scream
        // also uses or a device that should not be offered, and naming it
        // would make the second look like the first.
        let Some(announcement) =
            discovery::decode_announce(&datagram[..length], &invite.nonce, &invite.code)
        else {
            continue;
        };

        let id = discovery::audio_address(from, &announcement).to_string();
        return Ok(Some(DiscoveredDevice {
            name: announcement.name,
            address: id.clone(),
            id,
        }));
    }

    Ok(None)
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

/// Whether datagrams to `target` leave over a wired interface.
///
/// Asked of the routing table rather than of the address, because USB
/// tethering has no reserved range: a phone here handed out 10.114.89.x, which
/// looks like any other private network. Connecting a throwaway UDP socket
/// sends nothing -- it only makes the kernel choose a source address -- and
/// that address identifies the interface, which the adapter list can then
/// name.
///
/// False on any doubt. Claiming a wired link that is not one would have the
/// receiver hold ten milliseconds against Wi-Fi's jitter, which underruns; the
/// reverse merely costs latency, which is what this already does.
#[cfg(windows)]
fn is_wired_route(target: SocketAddr) -> bool {
    if target.ip().is_multicast() {
        return false;
    }

    let Some(local) = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .ok()
        .and_then(|probe| {
            probe
                .connect(target)
                .ok()
                .and_then(|()| probe.local_addr().ok())
        })
    else {
        return false;
    };

    adapters::enumerate().is_ok_and(|tethers| {
        tethers.iter().any(|adapter| {
            adapter.local == local.ip() && adapters::looks_like_tether(&adapter.description)
        })
    })
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
    // Declared once, at the top, because it cannot change mid-session: the
    // route to the target was chosen when the socket was bound.
    let mut packetizer = Packetizer::new(format, wire).on_wired_link(is_wired_route(target));
    let mut pcm = Vec::with_capacity(1 << 16);
    let mut counters = telemetry::Accumulator::new(format);
    let mut consecutive_failures = 0_u32;

    // Everything the UI says about the far end comes from here. Without it the
    // sender can only describe itself, which is how it came to report a
    // working session with nothing on the network.
    let mut round_trip = RoundTrip::new();
    let mut feedback_buffer = [0_u8; FEEDBACK_BYTES];
    let started = std::time::Instant::now();

    // Set here rather than trusted from the caller. This loop reads the socket
    // for the receiver's reports, and on a blocking socket that read waits
    // forever the moment there is nothing to read. A caller that built the
    // socket itself sent exactly one packet and then stopped, which looked
    // from the far end like a sender that had crashed.
    if let Err(error) = socket.set_nonblocking(true) {
        counters.record_send_error(&format!("could not set the socket non-blocking: {error}"));
    }

    while !stop.load(Ordering::Relaxed) {
        pcm.clear();
        let frames = match capture.read(&mut pcm) {
            Ok(frames) => {
                if consecutive_failures > 0 {
                    // Reading again after a run of failures means the device
                    // came back. Clearing the error matters as much as
                    // recording it did: a status stuck on a fault that has
                    // resolved is worse than no status.
                    if let Ok(mut current) = snapshot.lock() {
                        current.clear_error();
                    }
                }
                consecutive_failures = 0;
                frames
            }
            Err(error) => {
                counters.record_capture_error(&error.to_string());
                consecutive_failures += 1;

                // Unplugging a headset or switching the default output kills
                // the stream, and WASAPI has no way to move a client to
                // another endpoint: the only recovery is a new client. Before
                // this, the user had to stop and start the bridge by hand,
                // having been told only that capture failed.
                if consecutive_failures >= FAILURES_BEFORE_REOPEN {
                    match reopen(capture) {
                        Ok(()) => {
                            counters.record_reopen();
                            consecutive_failures = 0;
                            // The new device may be at a different rate, and
                            // the packetizer is built around the old one.
                            // Sending the new audio through it would play it
                            // at the wrong speed.
                            let reopened = capture.format();
                            if reopened != format {
                                if let Ok(mut current) = snapshot.lock() {
                                    current.mark_error(
                                        "the new playback device runs at a different format;                                          restart the bridge",
                                    );
                                }
                                return;
                            }
                            if let Ok(mut current) = snapshot.lock() {
                                current.clear_error();
                            }
                            continue;
                        }
                        Err(reopen_error) => {
                            if consecutive_failures == FAILURES_BEFORE_ERROR {
                                if let Ok(mut current) = snapshot.lock() {
                                    current.mark_error(&reopen_error);
                                }
                            }
                            std::thread::sleep(REOPEN_BACKOFF);
                            continue;
                        }
                    }
                }

                // Backing off keeps the loop from spinning at full speed
                // between attempts.
                std::thread::sleep(BACKOFF);
                continue;
            }
        };

        if frames > 0 {
            let result = packetizer.push(&pcm, |datagram| {
                // Noted before the send rather than after, so a slow send is
                // charged to the round trip it actually delayed.
                if let Ok(packet) = sonduit_core::packet::SonduitPacket::decode(datagram) {
                    round_trip
                        .record_send(packet.timestamp_frames, started.elapsed().as_nanos() as u64);
                }
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

        // Drain whatever the receiver has sent back. Non-blocking, so an
        // absent receiver costs one failed read per capture block and nothing
        // else.
        while let Ok((length, _from)) = socket.recv_from(&mut feedback_buffer) {
            let Some(report) = Feedback::decode(&feedback_buffer[..length]) else {
                continue;
            };
            let measured =
                round_trip.observe_echo(report.echo, started.elapsed().as_nanos() as u64);
            counters.record_feedback(report, round_trip.round_trip_ms().or(measured));
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

/// Replace a dead capture client with a fresh one on the current default
/// endpoint.
///
/// Opened in place so the caller keeps its `&mut`, and so the old client is
/// dropped, releasing the endpoint, before the new one asks for it.
#[cfg(windows)]
fn reopen(capture: &mut sonduit_capture_win::LoopbackCapture) -> Result<(), String> {
    use sonduit_capture_win::{open, CaptureMode};

    let replacement = open(CaptureMode::EndpointLoopback, CAPTURE_PERIOD_MS)
        .map_err(|error| format!("could not reopen the playback device: {error}"))?;
    *capture = replacement;
    Ok(())
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
    fn a_fresh_state_has_no_invite_to_pair_against() {
        // Waiting for a phone before a QR has been shown must fail rather than
        // wait on a code nobody has seen.
        assert!(BridgeState::default().invite().is_none());
    }

    #[test]
    fn a_phone_that_scanned_the_invite_is_accepted_and_located_by_its_source_address() {
        // The whole QR path, minus the camera: build the invite the panel
        // would show, have a stand-in phone parse it and answer the way the
        // FFI does, and check the desktop learns where it is. Over loopback,
        // on a port of its own, so it neither needs a network nor collides
        // with a real discovery listener.
        use sonduit_transport::invite::Invite;
        use std::net::Ipv4Addr;

        const PORT: u16 = 45_011;

        let nonce = scan_nonce();
        let invite = Invite::new(
            // A routable address, because an invite refuses to carry loopback:
            // it is never somewhere a phone could send. The stand-in phone
            // below sends over loopback regardless, which is what a real one
            // does from another machine.
            &[Ipv4Addr::new(10, 10, 0, 61)],
            PORT,
            PairingCode::from_seed(seed_from(&nonce)),
            nonce,
        )
        .expect("a routable address makes a valid invite");

        let payload = invite.to_payload();
        let phone = std::thread::spawn(move || {
            let scanned = Invite::parse(&payload).expect("the phone must be able to read it");
            let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .expect("an ephemeral loopback socket");
            let datagram =
                discovery::encode_announce("Pixel 7a", 4010, &scanned.nonce, &scanned.code);

            // Repeated because the listener may not be bound yet, and a single
            // datagram lost to that race would make this test flaky rather
            // than failing.
            for _ in 0..40 {
                let _ = socket.send_to(
                    &datagram,
                    SocketAddr::from((Ipv4Addr::LOCALHOST, scanned.port)),
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        });

        let state = BridgeState::default();
        *state.invite.lock().expect("a fresh state is not poisoned") = Some(invite);

        let session = state.pairing_session().expect("the listener must bind");
        let found = await_pairing(&session).expect("the wait itself cannot fail");
        let _ = phone.join();

        let device = found.expect("an announcement keyed by the invite must be accepted");
        assert_eq!(device.name, "Pixel 7a");
        // The address comes from the datagram, the port from the announcement.
        assert_eq!(device.address, "127.0.0.1:4010");
    }

    #[test]
    fn a_second_invite_does_not_collide_with_the_first() {
        // Showing the code, closing the dialog and showing it again used to
        // fail with "only one usage of each socket address": the first wait
        // held the discovery port for the rest of its ninety-second window.
        use sonduit_transport::invite::Invite;
        use std::net::Ipv4Addr;

        const PORT: u16 = 45_012;

        let make = || {
            let nonce = scan_nonce();
            Invite::new(
                &[Ipv4Addr::new(10, 10, 0, 61)],
                PORT,
                PairingCode::from_seed(seed_from(&nonce)),
                nonce,
            )
            .expect("a routable address makes a valid invite")
        };

        let state = BridgeState::default();
        *state.invite.lock().expect("not poisoned") = Some(make());
        let first = state.pairing_session().expect("the first listener binds");

        *state.invite.lock().expect("not poisoned") = Some(make());
        let second = state
            .pairing_session()
            .expect("a second invite must not be refused the port");

        assert!(
            first.superseded(),
            "the first wait must stand down once a second invite claims the port"
        );
        assert!(!second.superseded(), "the newest wait is the live one");

        state.cancel_pairing();
        assert!(
            second.superseded(),
            "closing the dialog must stop the wait rather than leave it listening"
        );
    }

    #[test]
    fn a_superseded_wait_returns_without_a_device() {
        // And returns quickly: the whole point is that the port comes free
        // long before the ninety seconds are up.
        use sonduit_transport::invite::Invite;
        use std::net::Ipv4Addr;

        const PORT: u16 = 45_013;

        let nonce = scan_nonce();
        let invite = Invite::new(
            &[Ipv4Addr::new(10, 10, 0, 61)],
            PORT,
            PairingCode::from_seed(seed_from(&nonce)),
            nonce,
        )
        .expect("a routable address makes a valid invite");

        let state = BridgeState::default();
        *state.invite.lock().expect("not poisoned") = Some(invite);
        let session = state.pairing_session().expect("the listener binds");

        let started = Instant::now();
        state.cancel_pairing();
        let found = await_pairing(&session).expect("the wait itself cannot fail");

        assert!(found.is_none(), "a cancelled wait pairs nothing");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancelling must be felt at once, not after the full window"
        );
    }

    #[test]
    fn an_announcement_keyed_by_a_stale_invite_is_ignored() {
        // Showing a new code must retire the old one. Without this a photograph
        // of any invite ever displayed would keep working.
        use sonduit_transport::invite::Invite;
        use std::net::Ipv4Addr;

        let code = PairingCode::from_seed(1);
        let stale = discovery::encode_announce("Attacker", 4010, &[0x11; NONCE_BYTES], &code);
        let current = Invite {
            addresses: vec![Ipv4Addr::LOCALHOST],
            port: 45_012,
            code,
            nonce: [0x22; NONCE_BYTES],
        };

        assert_eq!(
            discovery::decode_announce(&stale, &current.nonce, &current.code),
            None
        );
    }

    #[test]
    fn every_one_of_the_first_eight_nonce_bytes_reaches_the_seed() {
        // A seed that ignored most of its input would leave the code
        // predictable from the clock, which is the one thing it must not be.
        let mut nonce = [0_u8; NONCE_BYTES];
        let base = seed_from(&nonce);

        for index in 0..8 {
            nonce[index] = 1;
            assert_ne!(seed_from(&nonce), base, "byte {index} is discarded");
            nonce[index] = 0;
        }
    }

    #[test]
    fn stopping_a_stopped_bridge_is_not_an_error() {
        // The window close handler calls this unconditionally.
        let state = BridgeState::default();
        assert!(stop(&state).is_ok());
        assert!(stop(&state).is_ok());
    }
}
