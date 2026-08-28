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
use sonduit_transport::feedback::Feedback;
use sonduit_transport::handshake::Offer;
use sonduit_transport::invite::Invite;
use sonduit_transport::packetize::Packetizer;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::roundtrip::RoundTrip;
use sonduit_transport::session::SessionSecret;
use sonduit_transport::{discovery, entropy, TransportError, Wire, DEFAULT_PORT};
use tauri::{AppHandle, Emitter};

pub mod adapters;
pub mod link;
pub mod migrate;
pub mod peer;
mod telemetry;

pub use link::{Link, LinkKind, LinkSwitch, Route};
pub use migrate::Preference;
pub use peer::Peer;
pub use telemetry::{Accumulator, BridgeSnapshot, SessionInfo, TelemetryView};

// Only the send loop touches the cipher, and the send loop is Windows-only.
#[cfg(windows)]
use sonduit_transport::sealed::{FeedbackOpener, Sealer, SEALED_FEEDBACK_BYTES};

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

/// How often the link watcher wakes, as against how often it looks.
///
/// It sleeps in quarter-second slices so stopping the bridge is felt at once,
/// and does its work every [`migrate::POLL_SECONDS`]. Sleeping the whole poll
/// would make the stop button take two seconds to answer.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// Polls between broadcasts looking for the peer away from the cable.
///
/// Only runs while the session is on a cable, and only to keep a retreat
/// available: five polls is ten seconds, which is far shorter than a DHCP
/// lease and costs three small datagrams.
const WIRELESS_REFRESH_POLLS: u32 = 5;

/// How long the desktop waits for a phone's half of the key agreement.
///
/// The offer goes out three times and this bounds the listen after it. Short,
/// because both ends are already talking and the reply is one datagram: over a
/// cable it is back in a millisecond, and over Wi-Fi a device that has not
/// answered in six hundred milliseconds is not going to.
///
/// A device that does not complete this is not offered to the user at all. It
/// cannot be sent audio -- the receiver refuses anything not sealed under a key
/// it holds -- so listing it would be listing something that cannot work.
const KEY_AGREEMENT_WINDOW: Duration = Duration::from_millis(600);

/// Times the key offer is sent before the listen begins.
///
/// Same reasoning as the probe it follows: a single datagram after an idle
/// period is regularly dropped on Wi-Fi, and three cost nothing.
///
/// Repeating it is only safe because the responder answers a repeated offer
/// with the bytes it answered the first copy with. That is a property of the
/// peer and not of this end, so [`agree_key`] checks it rather than assuming
/// it: see [`BridgeError::KeyDisagreement`].
const KEY_OFFERS: u32 = 3;

/// How long the desktop keeps listening after the first accept verifies.
///
/// The offers go out back to back, so their answers arrive within a round trip
/// of each other and this is wide enough to see all of them on any link this
/// product runs over. It is spent once per pairing, behind a dialog the user
/// is already waiting on, and it is what turns a peer that answered one offer
/// with two keys into a refusal at the pairing rather than a session in which
/// nothing is ever played.
const KEY_ACCEPT_GRACE: Duration = Duration::from_millis(50);

/// Consecutive capture blocks whose send failed before the session retreats.
///
/// Five, so about fifty milliseconds at a ten millisecond capture period.
/// One refusal is ordinary -- a full send buffer, a momentary reconfiguration
/// -- and treating it as a dead link would make the session hop on every busy
/// moment. Five in a row with nothing in between is an interface that has
/// stopped accepting datagrams, which is what pulling the cable looks like
/// from here, and it is seen roughly forty times faster than an adapter walk
/// would see it.
const RETREAT_AFTER_FAILURES: u32 = 5;

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

/// A device that answered, proved it holds the code, and agreed a key.
///
/// The two halves are separate types on purpose. [`DiscoveredDevice`] is
/// `Serialize` and goes to the webview; the secret is not and cannot, because
/// the field is private to this module and there is no accessor that hands it
/// out. The only way key material reaches the rest of the application is
/// through [`BridgeState::remember`], into a [`Peer`], and out again on the
/// capture thread.
#[derive(Debug)]
pub struct PairedDevice {
    /// What the user sees and picks from.
    pub device: DiscoveredDevice,
    /// What audio to this device is keyed from.
    secret: SessionSecret,
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
    /// The transport preference from settings: `auto`, `wifi` or `usb`.
    ///
    /// Optional, and absent means `auto`. See [`migrate::Preference`], which
    /// is the only thing that reads it.
    pub preferred_transport: Option<String>,
    /// Which output endpoint to tap, as an id from [`endpoints`].
    ///
    /// Absent means the console-role default, which is what Windows itself
    /// would hand a new stream. An id naming a device that has since gone
    /// falls back to that same default; the session reports the endpoint it
    /// actually opened, so the panel is not left claiming the chosen one.
    pub capture_device_id: Option<String>,
}

/// A render endpoint the user can pick from.
///
/// A separate type from `sonduit_capture_win::Endpoint` on purpose: the
/// capture crate is not the place to learn what the webview's IPC serialises
/// into, and this is the layer that owns that question.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioEndpoint {
    /// Stable identifier, which is what a session is started against.
    pub id: String,
    /// Name to show.
    pub name: String,
    /// Whether Windows currently treats this as the default output.
    pub is_default: bool,
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

    /// A session was asked for against a device this process has not paired
    /// with, so there is no key to encrypt the audio under.
    ///
    /// Refused rather than sent in the clear. See the module documentation on
    /// [`start`] for why that is the honest answer and not an inconvenience.
    #[error("pair with the phone before starting: audio is only sent encrypted")]
    NotPaired,

    /// The operating system's random source could not be read.
    ///
    /// Fatal to a session by design: the alternative is a key pair from a
    /// predictable seed, which looks exactly like a working one.
    #[error("no session key could be generated: {0}")]
    NoEntropy(String),

    /// The phone answered one key offer with two different keys.
    ///
    /// The offer is sent [`KEY_OFFERS`] times for loss tolerance and every
    /// copy is the same datagram, so every answer has to be the same datagram
    /// too. A peer that answers each copy afresh keeps the key it minted last
    /// while this end keeps the one that arrived first: both ends report a
    /// pairing, and the receiver then refuses every packet of the session.
    ///
    /// Raised rather than passed over in silence, because the alternative is
    /// what shipped: eighteen thousand packets sent, eighteen thousand
    /// refused, and a panel saying the session was encrypted.
    ///
    /// Nobody outside the pairing can provoke it. A second accept only counts
    /// if its tag verifies under the six-digit code, and an attacker able to
    /// forge that holds the code already.
    #[error("the phone answered the key agreement with two different keys; it is running an incompatible build")]
    KeyDisagreement,
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

/// The cipher state of one encrypted session, built before any thread is.
///
/// Both halves are made here rather than on the capture thread because both
/// need a fresh random salt and reading the system's random source can fail.
/// A failure has to reach the button the user pressed; on the capture thread
/// there would be no caller left to fail to, and the only options would be a
/// session that is silently in the clear or one that dies a moment after it
/// started.
///
/// One per session and no `Clone`: the sealer owns the packet counter, and a
/// counter that can be copied is a nonce that can be repeated.
///
/// Public only because [`capture_and_follow`] is, for the diagnostic examples.
/// There is deliberately no public constructor: the only thing that can build
/// one is [`start`], from a peer this process paired with, so an example
/// cannot manufacture a session key and nor can anything else.
#[cfg(windows)]
pub struct Keying {
    /// Seals the audio. Moved into the packetizer when the loop starts.
    sealer: Sealer,
    /// Opens the receiver's reports. A keyed sender refuses a cleartext one:
    /// otherwise anybody on the network could drive the numbers the user is
    /// shown and keep a dead session looking alive.
    feedback: FeedbackOpener,
}

#[cfg(windows)]
impl Keying {
    /// Key a session from the master secret agreed with the peer.
    ///
    /// Two salts, one per direction, each fresh for this stream. The audio and
    /// feedback keys are derived under different labels as well, so a report
    /// can never be replayed into the audio path or the reverse.
    ///
    /// # Errors
    /// Returns [`BridgeError::NoEntropy`] when the salts cannot be drawn.
    pub(crate) fn for_peer(secret: &SessionSecret) -> Result<Self, BridgeError> {
        let salt =
            entropy::stream_salt().map_err(|error| BridgeError::NoEntropy(error.to_string()))?;
        Ok(Self {
            sealer: Sealer::new(secret, salt),
            feedback: FeedbackOpener::new(secret.clone()),
        })
    }
}

/// A running session, owned by the Tauri state.
struct Running {
    stop: Arc<AtomicBool>,
    audio: Option<std::thread::JoinHandle<()>>,
    reporter: Option<std::thread::JoinHandle<()>>,
    /// The link watcher, when the session has a peer it can prove.
    watcher: Option<std::thread::JoinHandle<()>>,
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
    /// Devices this process has authenticated, and the code that did it.
    ///
    /// The credential is what makes automatic link switching possible at all:
    /// without something to prove the phone on the cable is the phone the
    /// session is streaming to, availability is the only evidence there is,
    /// and availability would send the machine's audio to a stranger's handset
    /// charging off the same laptop. A session whose target is not in here
    /// does not migrate. See [`peer`].
    peers: Mutex<Vec<Peer>>,
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

    /// Record that `devices` proved they hold `code` and agreed a key.
    ///
    /// Called after a scan and after a pairing, which are the only two ways a
    /// device is ever selected. Replacing rather than appending for the same
    /// address, because the newest proof is the one that is still true -- and
    /// with encryption that matters more than it did: the old entry's key is
    /// the key the phone has just replaced, so keeping it would key a session
    /// from a pairing that is over.
    ///
    /// This is the only path key material takes into the application, and the
    /// list it writes to never leaves this process.
    pub fn remember(&self, devices: &[PairedDevice], code: &PairingCode) {
        let Ok(mut known) = self.peers.lock() else {
            return;
        };
        for paired in devices {
            let Ok(address) = paired.device.id.parse::<SocketAddr>() else {
                continue;
            };
            known.retain(|peer| peer.address != address);
            known.push(Peer {
                address,
                name: paired.device.name.clone(),
                code: code.clone(),
                secret: paired.secret.clone(),
            });
        }
    }

    /// The credential for the device at `target`, if this process proved one.
    #[must_use]
    pub fn peer_at(&self, target: SocketAddr) -> Option<Peer> {
        self.peers
            .lock()
            .ok()?
            .iter()
            .find(|peer| peer.address == target)
            .cloned()
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

    /// The code this wait authenticates against.
    ///
    /// Handed out so the caller can remember it alongside the device it
    /// paired: it is the credential every later check of "is that the same
    /// phone" is made against, and the invite it came from is retired as soon
    /// as the dialog closes.
    #[must_use]
    pub fn code(&self) -> PairingCode {
        self.invite.code.clone()
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

/// Run the key agreement with a device that has just proved it holds `code`.
///
/// Two datagrams on the socket the announcement arrived on: this end's public
/// key, tagged with the code and bound to the same nonce, and the phone's
/// reply. Both ends then hold a master secret an eavesdropper cannot recover
/// even after brute-forcing the six digits, which is the whole reason the
/// agreement exists rather than a KDF over the code. See ADR-009.
///
/// `Ok(None)` means the device did not answer, or answered with something that
/// did not verify. Those are deliberately the same outcome: neither is a device
/// this desktop can send audio to, and telling them apart would only invite a
/// caller to treat one of them as recoverable.
///
/// # Why the listen does not stop at the first accept
///
/// The offer goes out [`KEY_OFFERS`] times and the copies are identical, so a
/// correct responder answers them all with one datagram repeated -- that is
/// what `sonduit_transport::handshake::Responder` is for. This end cannot tell
/// from one accept which key the peer kept, so rather than assume, it listens
/// on for [`KEY_ACCEPT_GRACE`] and requires the answers to agree. Two
/// different accepts mean the peer minted a key pair per copy, and the session
/// that would follow is one where every packet is refused.
///
/// # Errors
/// Returns [`BridgeError::NoEntropy`] when the system's random source cannot be
/// read. That is not a device problem and must not be reported as one: it
/// affects every pairing, and continuing would mean an X25519 key pair from a
/// seed somebody can guess.
///
/// Returns [`BridgeError::KeyDisagreement`] when the peer answered the same
/// offer with two different keys.
fn agree_key(
    socket: &UdpSocket,
    peer: SocketAddr,
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Result<Option<SessionSecret>, BridgeError> {
    let seed = entropy::key_seed().map_err(|error| BridgeError::NoEntropy(error.to_string()))?;
    let offer = Offer::new(seed, *nonce, code.clone());
    let datagram = offer.datagram();

    let mut left = false;
    for _ in 0..KEY_OFFERS {
        left |= socket.send_to(&datagram, peer).is_ok();
    }
    if !left {
        return Ok(None);
    }

    // The accept is looked for without spending the exchange on the first
    // datagram that arrives: one forged reply from anyone on the network would
    // otherwise be enough to make every pairing fail.
    let deadline = Instant::now() + KEY_AGREEMENT_WINDOW;
    let mut buffer = [0_u8; 256];
    let mut found: Option<Vec<u8>> = None;
    // Set when the first accept verifies, so the peer's other answers are
    // collected rather than left unread in the receive buffer.
    let mut settle_by: Option<Instant> = None;
    let mut disagreed = false;
    // Restored below. The caller's timeout is longer than the settle window,
    // and leaving it in place would make the window cost whatever that
    // happens to be rather than the fifty milliseconds it says.
    let caller_timeout = socket.read_timeout().ok().flatten();

    loop {
        let now = Instant::now();
        if now >= deadline || settle_by.is_some_and(|until| now >= until) {
            break;
        }
        let Ok((length, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        // The tag is what decides; the address only saves the work of checking
        // datagrams that plainly belong to another exchange.
        if from.ip() != peer.ip() {
            continue;
        }
        if !offer.is_our_accept(&buffer[..length]) {
            continue;
        }
        match &found {
            // A datagram duplicated by the network is the same bytes again,
            // and is ordinary.
            Some(first) if first.as_slice() == &buffer[..length] => {}
            Some(_) => {
                disagreed = true;
                break;
            }
            None => {
                found = Some(buffer[..length].to_vec());
                settle_by = Some(Instant::now() + KEY_ACCEPT_GRACE);
                let _ = socket.set_read_timeout(Some(KEY_ACCEPT_GRACE));
            }
        }
    }

    if settle_by.is_some() {
        let _ = socket.set_read_timeout(caller_timeout);
    }
    if disagreed {
        return Err(BridgeError::KeyDisagreement);
    }

    Ok(found.and_then(|accept| offer.accept(&accept)))
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
pub fn await_pairing(session: &PairingSession) -> Result<Option<PairedDevice>, BridgeError> {
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

        // The key agreement runs here, on the same socket and against the same
        // nonce, before the device is reported as paired. A phone that
        // announces itself and then cannot agree a key is not a phone this
        // desktop can stream to, so it is not offered: the receiver refuses
        // anything not sealed under a key it holds, and a device in the list
        // that cannot be sent audio is a device that looks broken later.
        let Some(secret) = agree_key(&session.socket, from, &invite.nonce, &invite.code)? else {
            continue;
        };

        let id = discovery::audio_address(from, &announcement).to_string();
        return Ok(Some(PairedDevice {
            device: DiscoveredDevice {
                name: announcement.name,
                address: id.clone(),
                id,
            },
            secret,
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
/// Every device that answers is then taken through the key agreement, and one
/// that does not complete it is dropped from the list. That is not tidiness:
/// a receiver that holds no key for this desktop refuses everything it sends,
/// so a device listed without one is a device the user can select and then
/// watch do nothing.
///
/// # Errors
/// Returns [`BridgeError::BadPairingCode`] for a code that is not six digits,
/// [`BridgeError::Network`] when the socket cannot be bound or the broadcast is
/// refused, and [`BridgeError::NoEntropy`] when no key can be generated at all.
pub fn discover(code: &str) -> Result<Vec<PairedDevice>, BridgeError> {
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

    let mut answered: Vec<(SocketAddr, DiscoveredDevice)> = Vec::new();
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
        if answered.iter().any(|(_, device)| device.id == id) {
            continue;
        }
        answered.push((
            from,
            DiscoveredDevice {
                name: announcement.name.clone(),
                address: id.clone(),
                id,
            },
        ));
    }

    // The key agreement runs after the scan window rather than inside it. Each
    // one is a round trip with a device that may not answer, and running them
    // in the collection loop would spend the scan window waiting on the first
    // phone instead of hearing the second.
    let mut found = Vec::with_capacity(answered.len());
    for (from, device) in answered {
        match agree_key(&socket, from, &nonce, &code)? {
            Some(secret) => found.push(PairedDevice { device, secret }),
            // Dropped in silence, exactly as an announcement that does not
            // verify is: it is either a device running an older build or one
            // that is not what it claims, and naming the second would make it
            // look like the first.
            None => continue,
        }
    }

    Ok(found)
}

/// List the output devices a session can be started against.
///
/// # Why this gets a thread of its own
///
/// The same reason capture does, and it is not the same thread. WASAPI is COM,
/// and the enumerator has to be created on a thread whose apartment it can
/// live in. The thread a Tauri command runs on is the one hosting the webview,
/// which is already in a single-threaded apartment; asking it for a
/// multi-threaded one fails, and uninitialising it afterwards would take the
/// webview's own apartment down with it. A thread spawned and joined here
/// starts with no apartment at all, so the guard inside the capture crate is
/// free to make and unmake one. Enumeration happens when a settings page
/// opens, not per audio block, so a thread per call costs nothing that matters.
///
/// # Errors
/// Returns [`BridgeError::Capture`] when the enumerator cannot be created or
/// the machine has no active render device.
#[cfg(windows)]
pub fn endpoints() -> Result<Vec<AudioEndpoint>, BridgeError> {
    let listing = std::thread::Builder::new()
        .name("sonduit-endpoints".into())
        .spawn(|| {
            sonduit_capture_win::enumerate_endpoints().map(|found| {
                found
                    .into_iter()
                    .map(|endpoint| AudioEndpoint {
                        id: endpoint.id,
                        name: endpoint.name,
                        is_default: endpoint.is_default,
                    })
                    .collect::<Vec<_>>()
            })
        })
        .map_err(|error| BridgeError::Capture(error.to_string()))?
        .join()
        .map_err(|_| BridgeError::Capture("the device listing thread panicked".into()))?;

    listing.map_err(|error| BridgeError::Capture(error.to_string()))
}

/// Off Windows there are no endpoints to choose between.
///
/// # Errors
/// Always returns [`BridgeError::Capture`].
#[cfg(not(windows))]
pub fn endpoints() -> Result<Vec<AudioEndpoint>, BridgeError> {
    Err(BridgeError::Capture(
        "system audio capture is implemented for Windows only".into(),
    ))
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

    // Established once, here, and never derived a second time. The header flag
    // the receiver reads and the word the user reads are both this value.
    let home = Route {
        target,
        bind,
        kind: link::for_route(target, bind),
    };
    let opening =
        Link::bind(home.clone()).map_err(|error| BridgeError::Network(error.to_string()))?;
    let switch = Arc::new(LinkSwitch::new(home.clone()));

    // Migration needs something that can prove the phone on the other path is
    // this phone. A target that was never authenticated -- typed by hand, or
    // the multicast group -- has nothing to prove it with, so that session
    // stays where the user put it. See the module docs in `peer`.
    //
    // The same record carries the master secret, so this one lookup answers
    // both "may this session follow the link" and "what is it keyed from".
    let follow = (home.kind != LinkKind::Multicast)
        .then(|| state.peer_at(target))
        .flatten();

    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::clone(&state.snapshot);
    let wire = if options.scream_compatible {
        Wire::Scream
    } else {
        Wire::Sonduit
    };

    // Whether this session can be encrypted is settled here, before a thread
    // exists, so the answer lands on the button the user pressed.
    //
    // A Sonduit session with no key is refused rather than sent in the clear.
    // There is no pairing this refusal can strand: the list of paired devices
    // lives in this process and nothing persists it, so every session that
    // has ever worked was paired inside the run that started it. What is
    // refused is a target that was never authenticated at all -- an address
    // typed by hand, or the multicast group -- and audio to one of those was
    // never going anywhere provable in the first place.
    //
    // Scream is the one exception and it is a property of that protocol, not
    // a choice: its five-byte header has no version field and nowhere to put
    // a tag, and the receiver it exists for is a third-party driver with no
    // key. That session runs in the clear and says so; see `SessionInfo`.
    let keying = match (wire, follow.as_ref()) {
        (Wire::Scream, _) => None,
        (Wire::Sonduit, Some(peer)) => Some(Keying::for_peer(&peer.secret)?),
        (Wire::Sonduit, None) => return Err(BridgeError::NotPaired),
    };
    let encrypted = keying.is_some();

    // Capture is opened on the thread that will use it, not here. WASAPI is
    // COM: the interfaces are apartment-bound and the apartment is per-thread,
    // so a client opened here and moved would be used from a thread that never
    // initialised COM. The channel keeps the error reporting synchronous
    // anyway, so a failure still lands on the button the user pressed rather
    // than as a session that starts and then silently dies.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let chosen = options.capture_device_id.clone();

    let audio = {
        let stop = Arc::clone(&stop);
        let snapshot = Arc::clone(&snapshot);
        let switch = Arc::clone(&switch);
        std::thread::Builder::new()
            .name("sonduit-capture".into())
            .spawn(move || {
                let mut capture = match open(
                    CaptureMode::EndpointLoopback,
                    CAPTURE_PERIOD_MS,
                    chosen.as_deref(),
                ) {
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
                // The endpoint that was opened, not the one that was asked
                // for. They differ when the chosen device has gone, and the
                // panel saying which speakers the audio is really coming from
                // is the only way the user finds that out.
                let endpoint = capture.endpoint().name.clone();

                if ready_tx
                    .send(Ok((format, endpoint, capture.stopper())))
                    .is_err()
                {
                    // The caller gave up waiting, so there is nobody to send
                    // audio for.
                    return;
                }

                capture_and_follow(
                    &mut capture,
                    opening,
                    format,
                    wire,
                    &stop,
                    &snapshot,
                    Some(&switch),
                    keying,
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

    let info = SessionInfo::new(
        &endpoint,
        format,
        target,
        home.kind,
        options.scream_compatible,
        encrypted,
    );

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

    let watcher = follow.map(|peer| {
        let stop = Arc::clone(&stop);
        let switch = Arc::clone(&switch);
        let preference = Preference::parse(options.preferred_transport.as_deref());
        std::thread::Builder::new()
            .name("sonduit-link".into())
            .spawn(move || watch_link(&stop, &switch, &peer, home, preference))
    });
    let watcher = match watcher {
        Some(Ok(handle)) => Some(handle),
        // A session that plays but cannot follow the link is far better than
        // no session. The user is where they were before this existed.
        Some(Err(error)) => {
            crate::core::logging::log_message(
                "bridge",
                &format!("the link watcher could not start, staying on one link: {error}"),
            );
            None
        }
        None => None,
    };

    *guard = Some(Running {
        stop,
        audio: Some(audio),
        reporter: Some(reporter),
        watcher,
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

/// Watch for a better link, and for the one in use going away.
///
/// # Where the expensive work lives
///
/// All of it, here. Walking the adapter list is several syscalls and binding a
/// socket is one more, and none of that may happen on the thread that has to
/// put a packet on the wire every six milliseconds. So this thread does the
/// looking, the proving and the binding, and leaves a finished socket in the
/// [`LinkSwitch`]; the send loop's whole share of a migration is taking it.
///
/// # What one poll costs
///
/// One walk of the adapter list, every two seconds. A verification probe on
/// top of that only while a cable is present and the session is not on it, and
/// a broadcast probe every ten seconds only while the session *is* on a cable
/// and needs somewhere to retreat to. Nothing per packet, and nothing at all
/// on the capture thread.
fn watch_link(
    stop: &AtomicBool,
    switch: &LinkSwitch,
    peer: &Peer,
    home: Route,
    preference: Preference,
) {
    use crate::core::logging::log_message;

    let ticks_per_poll = (migrate::POLL_SECONDS * 1_000 / WATCH_TICK.as_millis() as u64).max(1);
    let mut policy = migrate::Policy::new(preference);
    // Where to retreat to. The route the session started on, when that was not
    // a cable: it is the address the user paired against and it is known good.
    let mut wireless = (home.kind != LinkKind::Wired).then_some(home);
    let mut ticks = 0_u64;
    let mut since_broadcast = 0_u32;
    // The route the standby socket was bound for, so a fallback that has moved
    // is re-bound rather than left pointing at a lease that has expired.
    let mut armed_for: Option<Route> = None;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(WATCH_TICK);
        ticks += 1;
        // A retreat the send loop took on its own does not wait out the poll.
        // It has already happened, and until this thread has re-armed there is
        // nothing left to retreat to a second time.
        let urgent = switch.took_retreat();
        if ticks < ticks_per_poll && !urgent {
            continue;
        }
        ticks = 0;
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let adapters = adapters::enumerate().unwrap_or_default();
        let Some(live) = switch.live() else {
            continue;
        };

        if live.kind == LinkKind::Wired {
            // Keep a way back. A session that started on the cable has never
            // been told where the phone is on the wireless network, and one
            // that started on Wi-Fi can outlive the lease it was told about.
            since_broadcast += 1;
            if wireless.is_none() || since_broadcast >= WIRELESS_REFRESH_POLLS {
                since_broadcast = 0;
                if let Some(found) = peer::find_elsewhere(peer, &scan_nonce(), |from| {
                    if link::is_tether_gateway(from.ip(), &adapters) {
                        LinkKind::Wired
                    } else {
                        LinkKind::Wireless
                    }
                }) {
                    wireless = Some(found);
                }
            }
        } else {
            since_broadcast = 0;
            wireless = Some(live.clone());
        }

        // Checked again here, and not only at the top: the broadcast above can
        // take three hundred milliseconds and the verification below another
        // three hundred, and stopping the bridge closes a window. Without this
        // the close would wait out both.
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let current_alive = link::route_alive(&live, &adapters);
        // Proving a candidate costs three datagrams and up to three hundred
        // milliseconds of this thread. Not worth spending every two seconds on
        // a link the preference forbids taking -- unless the link in use has
        // died, in which case the candidate is not an upgrade but the only
        // thing left.
        let wired = if preference.allows_upgrade() || !current_alive {
            verified_cable(&live, &adapters, peer)
        } else {
            None
        };

        let observation = migrate::Observation {
            current: live.kind,
            current_alive,
            wired,
            wireless: wireless.clone(),
        };

        let mut expected = live.kind;
        if let migrate::Decision::Move(route, reason) = policy.observe(&observation) {
            match Link::bind(route.clone()) {
                Ok(link) => {
                    log_message(
                        "bridge",
                        &format!(
                            "moving the session to {} over {} ({reason:?})",
                            route.target,
                            route.kind.label()
                        ),
                    );
                    expected = route.kind;
                    switch.offer(link);
                }
                // The interface went between deciding and binding. Nothing is
                // lost: the session is where it was, and the next poll sees
                // the same world two seconds later.
                Err(error) => {
                    log_message("bridge", &format!("could not bind {}: {error}", route.bind))
                }
            }
        }

        // The retreat the send loop is allowed to take without asking. Only
        // while the session is on a cable, because that is the failure that is
        // total and instantaneous; a wireless link degrading is something the
        // poll above is quite fast enough for.
        if expected == LinkKind::Wired {
            // Re-armed when the fallback has moved as well as when there is
            // none: the phone's address on the wireless network is a DHCP
            // lease, and a standby bound for the address it held an hour ago
            // is a retreat onto nothing.
            let stale = armed_for.as_ref() != wireless.as_ref();
            if !switch.armed() || stale {
                if let Some(route) = wireless.clone() {
                    match Link::bind(route.clone()) {
                        Ok(link) => {
                            switch.arm(link);
                            armed_for = Some(route);
                        }
                        Err(error) => log_message(
                            "bridge",
                            &format!("could not hold a fallback on {}: {error}", route.bind),
                        ),
                    }
                }
            }
        } else {
            // A standby bound for an interface that has since gone is worse
            // than none: the send loop would retreat onto nothing.
            switch.disarm();
            armed_for = None;
        }
    }

    switch.disarm();
}

/// The best cable to the peer, proved to be the peer, if there is one.
///
/// `None` while the session is already on a cable: there is nothing better to
/// move to, and probing would be a datagram spent on a question whose answer
/// changes nothing.
fn verified_cable(
    live: &Route,
    adapters: &[adapters::TetherAdapter],
    peer: &Peer,
) -> Option<Route> {
    if live.kind == LinkKind::Wired {
        return None;
    }

    // The list arrives ranked, so the adapter that both looks like a tether
    // and sits in the range Android usually hands out is asked first. That
    // ranking is the one remaining honest use of the 192.168.42/24
    // assumption: it orders candidates, it never decides what a link is.
    let candidate = adapters.iter().find(|adapter| {
        adapters::looks_like_tether(&adapter.description) && adapter.local != live.bind.ip()
    })?;

    peer::verify(
        &Route::over(candidate, peer.audio_port()),
        peer,
        &scan_nonce(),
    )
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
///
/// **This path sends in the clear.** It exists for the diagnostic examples in
/// `examples/`, each of which brings its own listener in the same process and
/// has no pairing to key from. It is not reachable from the application: a
/// session the user starts goes through [`start`], which refuses a Sonduit
/// stream it cannot encrypt.
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
    let route = Route::unbound(target, link::for_route(target, socket_bind(socket)));
    match socket.try_clone() {
        Ok(socket) => capture_and_follow(
            capture,
            Link { socket, route },
            format,
            wire,
            stop,
            snapshot,
            None,
            None,
        ),
        Err(error) => {
            if let Ok(mut current) = snapshot.lock() {
                current.mark_error(&format!("the sending socket could not be shared: {error}"));
            }
        }
    }
}

/// The local address a socket is bound to, or the unspecified one.
///
/// Only used to work out which link a caller-supplied socket is on. A socket
/// whose address cannot be read is treated as unbound, which sends the
/// question to the routing table instead.
#[cfg(windows)]
fn socket_bind(socket: &UdpSocket) -> SocketAddr {
    socket
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
}

/// The parts of the send loop a migration replaces, and nothing else.
///
/// Grouped so that adopting a link is one call rather than four assignments
/// that have to be kept in step, and so the two things that must *not* be
/// replaced -- the packetizer's sequence number and frame timestamp -- are
/// visibly carried across rather than accidentally preserved.
#[cfg(windows)]
struct Sending {
    link: Link,
    packetizer: Packetizer,
    round_trip: RoundTrip,
    format: Format,
    wire: Wire,
}

#[cfg(windows)]
impl Sending {
    /// Start sending, encrypted when a sealer was built for this session.
    ///
    /// The sealer is moved into the packetizer because it owns the packet
    /// counter, and the counter is what makes every nonce unique. There is
    /// deliberately no way to attach one later: a stream that started in the
    /// clear cannot become encrypted, it can only be a second stream.
    fn new(link: Link, format: Format, wire: Wire, sealer: Option<Sealer>) -> Self {
        let wired = link.route.kind.is_wired();
        let packetizer = match sealer {
            Some(sealer) => Packetizer::sealed(format, sealer),
            None => Packetizer::new(format, wire),
        };
        Self {
            link,
            packetizer: packetizer.on_wired_link(wired),
            round_trip: RoundTrip::new(),
            format,
            wire,
        }
    }

    /// Move the session onto a link the watcher has already bound.
    ///
    /// # Why this costs no audio
    ///
    /// The packetizer is moved through its builder rather than rebuilt, so the
    /// sequence number, the frame timestamp and the half-filled packet all
    /// continue. The receiver identifies a stream by exactly those, and it
    /// reads the source address off the datagram and replies there, so from
    /// its side one unbroken stream simply arrives from somewhere new. Nothing
    /// is dropped, nothing is resent, and no packet boundary moves.
    ///
    /// The only thing thrown away is the round-trip history, which describes a
    /// path that no longer exists. Its one-pole filter would take about four
    /// seconds to unlearn the old link, and in the meantime the latency figure
    /// would be an average of two different networks.
    /// The sealer travels with the packetizer, which is what keeps the stream
    /// one stream: the same salt, the same key and a counter that carries on
    /// rather than restarting at zero. Restarting it under the same key is
    /// exactly the nonce reuse the salt exists to prevent, so a migration must
    /// never build a fresh packetizer for a session that has one.
    fn adopt(&mut self, next: Link) {
        self.link = next;
        let placeholder = Packetizer::new(self.format, self.wire);
        let carried = std::mem::replace(&mut self.packetizer, placeholder);
        self.packetizer = carried.on_wired_link(self.link.route.kind.is_wired());
        self.round_trip = RoundTrip::new();
    }
}

/// Whether a send failure means the route has gone rather than merely stalled.
///
/// A full send buffer is ordinary and says nothing about the interface. Any
/// other socket error on a datagram send is the kernel saying it has nowhere
/// to put this, which after a cable comes out is what every send says.
fn route_is_down(error: &TransportError) -> bool {
    match error {
        TransportError::Io(io) => !matches!(
            io.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ),
        _ => false,
    }
}

/// The capture and send loop, with the link free to change underneath it.
///
/// `switch` is where the link watcher leaves a bound socket. `None` is a
/// session that stays where it started, which is what an example driving this
/// by hand wants and what a session with no provable peer gets.
/// `keying` is the session's cipher state, or `None` for a stream that is not
/// encrypted -- which is Scream compatibility and the diagnostic examples, and
/// nothing else. When it is present every datagram is sealed and every report
/// coming back must authenticate; when it is absent neither is, and there is
/// deliberately no path between the two states inside one session.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)] // one session's worth of state, all of it needed
pub fn capture_and_follow(
    capture: &mut sonduit_capture_win::LoopbackCapture,
    link: Link,
    format: Format,
    wire: Wire,
    stop: &AtomicBool,
    snapshot: &Mutex<BridgeSnapshot>,
    switch: Option<&LinkSwitch>,
    keying: Option<Keying>,
) {
    let (sealer, mut feedback_opener) = match keying {
        Some(Keying { sealer, feedback }) => (Some(sealer), Some(feedback)),
        None => (None, None),
    };
    let mut sending = Sending::new(link, format, wire, sealer);
    let mut pcm = Vec::with_capacity(1 << 16);
    let mut counters = telemetry::Accumulator::new(format);
    if feedback_opener.is_some() {
        // From zero rather than from blank, so a keyed session that has never
        // seen a forged report says so instead of saying nothing.
        counters.checking_reports();
    }
    let mut consecutive_failures = 0_u32;
    // Consecutive capture blocks whose send failed for a reason that means the
    // interface has gone. Separate from the capture failure count above: one
    // is the audio device and the other is the network, and they recover in
    // completely different ways.
    let mut route_failures = 0_u32;

    // Sized for the sealed report, which is the larger of the two. A buffer
    // sized for the cleartext one would truncate every sealed report and every
    // one of them would then fail to authenticate, which reads as an attack.
    let mut feedback_buffer = [0_u8; SEALED_FEEDBACK_BYTES];
    let started = std::time::Instant::now();

    // Set here rather than trusted from the caller. This loop reads the socket
    // for the receiver's reports, and on a blocking socket that read waits
    // forever the moment there is nothing to read. A caller that built the
    // socket itself sent exactly one packet and then stopped, which looked
    // from the far end like a sender that had crashed.
    if let Err(error) = sending.link.socket.set_nonblocking(true) {
        counters.record_send_error(&format!("could not set the socket non-blocking: {error}"));
    }

    while !stop.load(Ordering::Relaxed) {
        // Adopting whatever the watcher decided on. One relaxed load on every
        // iteration but the one that migrates, and never a blocking lock: the
        // one moment the watcher holds that mutex is the one moment this loop
        // must not wait, and missing an offer costs a single capture block.
        if let Some(switch) = switch {
            if let Some(offered) = switch.take_offer() {
                sending.adopt(offered);
                route_failures = 0;
                switch.set_live(sending.link.route.clone());
                if let Ok(mut current) = snapshot.lock() {
                    current.note_link(sending.link.route.target, sending.link.route.kind);
                }
            }
        }

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
                                // The reopen may have landed somewhere else:
                                // the chosen device can still be gone, in
                                // which case this is the fallback's name.
                                current.note_endpoint(&capture.endpoint().name);
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
            let Sending {
                link,
                packetizer,
                round_trip,
                ..
            } = &mut sending;
            let result = packetizer.push(&pcm, |datagram| {
                // Noted before the send rather than after, so a slow send is
                // charged to the round trip it actually delayed.
                //
                // Read out of the header rather than by decoding, because a
                // sealed datagram does not decode as a version 1 packet and
                // the timestamp field means the same thing in both. This end
                // wrote the value a moment ago; nothing here is trusting the
                // wire. See `sonduit_transport::sonduit_timestamp`.
                if let Some(timestamp) = sonduit_transport::sonduit_timestamp(datagram) {
                    round_trip.record_send(timestamp, started.elapsed().as_nanos() as u64);
                }
                link.socket
                    .send_to(datagram, link.route.target)
                    .map(|_| ())
                    .map_err(TransportError::from)
            });
            match result {
                Ok(()) => {
                    counters.record_sent(frames, packetizer.packets());
                    route_failures = 0;
                }
                Err(error) => {
                    counters.record_send_error(&error.to_string());
                    if route_is_down(&error) {
                        route_failures += 1;
                    } else {
                        route_failures = 0;
                    }
                }
            }
        }

        // The cable came out. The watcher would see this two seconds later off
        // the adapter list; this loop sees it in about fifty milliseconds,
        // because it is the thing being refused. The socket it moves to was
        // bound minutes ago, so taking it is a pointer swap and the audio does
        // not stop for it -- only the datagrams already lost to the dead
        // interface are gone.
        if route_failures >= RETREAT_AFTER_FAILURES {
            route_failures = 0;
            if let Some(switch) = switch {
                if let Some(standby) = switch.take_retreat() {
                    sending.adopt(standby);
                    switch.set_live(sending.link.route.clone());
                    if let Ok(mut current) = snapshot.lock() {
                        current.note_link(sending.link.route.target, sending.link.route.kind);
                    }
                }
            }
        }

        // Drain whatever the receiver has sent back. Non-blocking, so an
        // absent receiver costs one failed read per capture block and nothing
        // else.
        while let Ok((length, _from)) = sending.link.socket.recv_from(&mut feedback_buffer) {
            // A keyed session opens the report and refuses a cleartext one.
            // That refusal is the same downgrade defence the audio path has,
            // pointing the other way: a sender that still accepted version 1
            // reports would let anybody on the network drive the loss figure,
            // the buffer depth and the round trip the user is shown, and keep
            // a session that has died looking alive.
            let report = match feedback_opener.as_mut() {
                Some(opener) => match opener.open(&feedback_buffer[..length]) {
                    Ok(report) => Some(report),
                    Err(_) => {
                        counters.record_refused_report();
                        None
                    }
                },
                None => Feedback::decode(&feedback_buffer[..length]),
            };
            let Some(report) = report else {
                continue;
            };
            let measured = sending
                .round_trip
                .observe_echo(report.echo, started.elapsed().as_nanos() as u64);
            counters.record_feedback(report, sending.round_trip.round_trip_ms().or(measured));
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

/// Replace a dead capture client with a fresh one.
///
/// Opened in place so the caller keeps its `&mut`, and so the old client is
/// dropped, releasing the endpoint, before the new one asks for it.
///
/// It asks for the endpoint the session was started against, not the one it
/// happens to be on. A headset that was chosen, unplugged and plugged back in
/// is picked up again on the next attempt; settling on whatever the fallback
/// found would quietly turn a choice into a one-off.
#[cfg(windows)]
fn reopen(capture: &mut sonduit_capture_win::LoopbackCapture) -> Result<(), String> {
    use sonduit_capture_win::{open, CaptureMode};

    let replacement = open(
        CaptureMode::EndpointLoopback,
        CAPTURE_PERIOD_MS,
        capture.requested_endpoint(),
    )
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
    if let Some(handle) = running.watcher.take() {
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
    fn a_peer_answering_one_offer_with_two_keys_is_refused_rather_than_paired_wrongly() {
        // The safety net under KEY_OFFERS. Repeating the offer is only safe
        // because the peer answers a repeat with the bytes it answered before,
        // and that is a property of the peer's build, not of this one. So it
        // is checked: a peer that mints a key pair per copy is refused at the
        // pairing, where it can be reported, instead of producing a session
        // that reports itself encrypted and plays nothing.
        //
        // The stand-in here is the receiver as it was before it remembered
        // anything -- a fresh responder per datagram -- which is exactly the
        // build this refusal exists to name.
        use sonduit_transport::handshake;
        use sonduit_transport::session::SEED_BYTES;
        use std::net::Ipv4Addr;

        let nonce = [0x3C_u8; NONCE_BYTES];
        let code = PairingCode::parse("482913").expect("six digits");

        let listener = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        listener
            .set_read_timeout(Some(Duration::from_millis(500)))
            .expect("a fresh socket takes a timeout");
        let peer = listener.local_addr().expect("bound");

        let phone = {
            let code = code.clone();
            std::thread::spawn(move || {
                let mut datagram = [0_u8; 256];
                let mut answered = 0_u8;
                while answered < KEY_OFFERS as u8 {
                    let Ok((length, from)) = listener.recv_from(&mut datagram) else {
                        break;
                    };
                    let Some(answer) = handshake::Responder::new().answer(
                        &datagram[..length],
                        &[nonce],
                        &code,
                        [0x31 + answered; SEED_BYTES],
                    ) else {
                        continue;
                    };
                    answered += 1;
                    let _ = listener.send_to(&answer.accept, from);
                }
            })
        };

        let desktop = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        desktop
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("a fresh socket takes a timeout");

        let outcome = agree_key(&desktop, peer, &nonce, &code);
        let _ = phone.join();

        assert!(
            matches!(outcome, Err(BridgeError::KeyDisagreement)),
            "a peer that answered one offer with two keys was paired with anyway: {outcome:?}"
        );
        // And the caller's own timeout survives the settle window, because the
        // sockets in this module are reused for the audio that follows.
        assert_eq!(
            desktop.read_timeout().expect("readable"),
            Some(Duration::from_millis(200)),
            "the read timeout was not put back"
        );
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
            use sonduit_transport::handshake;
            use sonduit_transport::session::SEED_BYTES;

            let scanned = Invite::parse(&payload).expect("the phone must be able to read it");
            // The phone's own responder, so a repeated offer is answered the
            // way the FFI answers it. A stand-in that answered only the first
            // copy would be a receiver no real one resembles, and it is what
            // let a desktop and a phone ship holding different keys.
            let mut responder = handshake::Responder::new();
            let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .expect("an ephemeral loopback socket");
            socket
                .set_read_timeout(Some(Duration::from_millis(25)))
                .expect("a fresh socket takes a timeout");
            let datagram =
                discovery::encode_announce("Pixel 7a", 4010, &scanned.nonce, &scanned.code);
            let mut buffer = [0_u8; 256];
            let mut answered = 0_u8;

            // Repeated because the listener may not be bound yet, and a single
            // datagram lost to that race would make this test flaky rather
            // than failing. The announcement stops as soon as the desktop's
            // key offer arrives, which is the phone's own confirmation that it
            // was heard.
            for _ in 0..80 {
                let _ = socket.send_to(
                    &datagram,
                    SocketAddr::from((Ipv4Addr::LOCALHOST, scanned.port)),
                );
                let Ok((length, from)) = socket.recv_from(&mut buffer) else {
                    continue;
                };
                // The other half of what the FFI does: answer the offer with
                // this end's public key and keep the secret. A stand-in that
                // only announced would be testing a pairing that cannot carry
                // audio.
                //
                // Every copy of the offer is answered, and the seed offered
                // for each is a different one, so a responder that did not
                // remember what it answered would be visible here as two
                // different accepts rather than hidden behind a constant.
                answered += 1;
                if let Some(answer) = responder.answer(
                    &buffer[..length],
                    &[scanned.nonce],
                    &scanned.code,
                    [0x2C + answered; SEED_BYTES],
                ) {
                    let _ = socket.send_to(&answer.accept, from);
                    // The desktop sends its offer three times. Answering all
                    // three is the point; waiting for a fourth that is never
                    // coming is only slow.
                    if answered == 3 {
                        return;
                    }
                }
            }
        });

        let state = BridgeState::default();
        *state.invite.lock().expect("a fresh state is not poisoned") = Some(invite);

        let session = state.pairing_session().expect("the listener must bind");
        let found = await_pairing(&session).expect("the wait itself cannot fail");
        let _ = phone.join();

        let paired = found.expect("an announcement keyed by the invite must be accepted");
        assert_eq!(paired.device.name, "Pixel 7a");
        // The address comes from the datagram, the port from the announcement.
        assert_eq!(paired.device.address, "127.0.0.1:4010");
        // And the pairing carries a key, because a pairing that did not would
        // produce a device the session could not encrypt to and would refuse.
        assert!(
            format!("{:?}", paired.secret).contains('*'),
            "the secret must redact itself wherever it is printed"
        );
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
    fn a_full_send_buffer_is_not_mistaken_for_an_unplugged_cable() {
        // The send loop retreats on this, so a momentary stall counted as a
        // dead route would move the session for no reason at all.
        use std::io::ErrorKind;

        for kind in [ErrorKind::WouldBlock, ErrorKind::Interrupted] {
            let error = TransportError::Io(std::io::Error::new(kind, "busy"));
            assert!(!route_is_down(&error), "{kind:?} was called a dead route");
        }
    }

    #[test]
    fn an_interface_that_has_gone_is_recognised_from_the_send_that_failed() {
        // Pulling the cable makes every send fail like this, about forty times
        // sooner than an adapter walk would notice.
        use std::io::ErrorKind;

        for kind in [
            ErrorKind::NotConnected,
            ErrorKind::AddrNotAvailable,
            ErrorKind::PermissionDenied,
        ] {
            let error = TransportError::Io(std::io::Error::new(kind, "no route"));
            assert!(
                route_is_down(&error),
                "{kind:?} was not called a dead route"
            );
        }
    }

    #[test]
    fn a_codec_failure_is_not_a_dead_route() {
        // It is a bug in this process, and retreating onto another interface
        // would take the same bug with it.
        let error = TransportError::UnknownFormat(7);
        assert!(!route_is_down(&error));
    }

    /// A device as a completed pairing would leave it.
    ///
    /// The secret comes from a real handshake with fixed seeds, so what is
    /// stored is the shape a session is actually keyed from.
    fn paired_at(id: &str, name: &str) -> PairedDevice {
        PairedDevice {
            device: DiscoveredDevice {
                id: id.to_string(),
                name: name.to_string(),
                address: id.to_string(),
            },
            secret: peer::agreed_secret(),
        }
    }

    #[test]
    fn a_device_that_was_never_authenticated_has_no_credential_to_migrate_on() {
        // Which is the whole safety property: with nothing to prove the phone
        // on a cable is this phone, the session does not move at all.
        let state = BridgeState::default();
        assert!(state
            .peer_at("192.168.1.42:4010".parse().unwrap())
            .is_none());
    }

    #[test]
    fn a_scanned_device_is_remembered_with_the_code_that_proved_it() {
        let state = BridgeState::default();
        let code = PairingCode::parse("482913").unwrap();
        state.remember(&[paired_at("192.168.1.42:4010", "Pixel 7a")], &code);

        let peer = state
            .peer_at("192.168.1.42:4010".parse().unwrap())
            .expect("the device that was just scanned");
        assert_eq!(peer.name, "Pixel 7a");
        assert_eq!(peer.audio_port(), 4010);
        assert!(peer.code == code);

        // A different address is a different device, whatever the code was.
        assert!(state
            .peer_at("192.168.1.43:4010".parse().unwrap())
            .is_none());
    }

    #[test]
    fn scanning_again_replaces_the_credential_rather_than_stacking_them() {
        // The newest proof is the one that is still true, and two entries for
        // one address would leave which of them wins to iteration order.
        let state = BridgeState::default();
        state.remember(
            &[paired_at("192.168.1.42:4010", "Pixel 7a")],
            &PairingCode::parse("111111").unwrap(),
        );
        state.remember(
            &[paired_at("192.168.1.42:4010", "Pixel 7a")],
            &PairingCode::parse("222222").unwrap(),
        );

        let peer = state
            .peer_at("192.168.1.42:4010".parse().unwrap())
            .expect("still known");
        assert!(peer.code == PairingCode::parse("222222").unwrap());
        assert_eq!(
            state.peers.lock().unwrap().len(),
            1,
            "one address collected two credentials"
        );
    }

    #[test]
    fn an_unparseable_address_is_dropped_rather_than_remembered_as_a_peer() {
        // Nothing produces one today. If something ever does, a credential
        // filed under an address that cannot be compared is a credential that
        // silently never matches, which is worse than not having it.
        let state = BridgeState::default();
        state.remember(
            &[paired_at("not-an-address", "Pixel 7a")],
            &PairingCode::parse("482913").unwrap(),
        );
        assert!(state.peers.lock().unwrap().is_empty());
    }

    #[test]
    fn stopping_a_stopped_bridge_is_not_an_error() {
        // The window close handler calls this unconditionally.
        let state = BridgeState::default();
        assert!(stop(&state).is_ok());
        assert!(stop(&state).is_ok());
    }
}
