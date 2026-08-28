//! UniFFI surface exposing the shared core to the Android app.
//!
//! The Kotlin UI drives the bridge through this crate. The audio callback does
//! **not** cross this boundary: it stays entirely inside Rust and native code,
//! because calling into the JVM from a realtime callback can allocate, take VM
//! locks and stall on GC. Kotlin starts and stops a session and polls
//! telemetry; everything between those calls happens below the FFI.
//!
//! # Threads
//!
//! A [`Bridge`] owns one from the moment it is constructed: the thread that
//! answers discovery probes. That one is not part of a session, because the
//! user reading the pairing code off this screen has not started a session
//! yet and still has to be findable. `start` adds a second, blocked on
//! `recv_from` and decoding datagrams into the jitter buffer. Audio is pulled
//! by AAudio on a third thread it owns. The jitter buffer is the only thing
//! the last two touch, and the audio callback only ever `try_lock`s it.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sonduit_core::drift::{DriftConfig, DriftEstimator};
use sonduit_core::format::Format;
use sonduit_core::jitter::{JitterBuffer, JitterConfig, LinkWatch, PushOutcome, Transport};
use sonduit_core::pacing::{drain_allowance, PacingConfig};
use sonduit_core::packet::{ScreamPacket, SonduitPacket};
use sonduit_core::ratio::{RatioConfig, RatioController};
use sonduit_core::resample::DriftResampler;
use sonduit_playback_android::{drain_packet, JitterSource};
use sonduit_transport::feedback::{Feedback, FEEDBACK_INTERVAL_MS};
use sonduit_transport::invite::Invite;
use sonduit_transport::pairing::{PairingCode, NONCE_BYTES};
use sonduit_transport::sealed::{FeedbackSealer, Opener, SEALED_FEEDBACK_BYTES};
use sonduit_transport::session::SessionSecret;
use sonduit_transport::{classify, discovery, entropy, handshake, Wire, DEFAULT_PORT};

uniffi::setup_scaffolding!();

/// Send a line to logcat.
///
/// The first session on real hardware showed a jitter buffer filling to its
/// 1536 ms ceiling with the correction pinned at its limit, and logcat had
/// nothing in it at all. Without this there is no way to tell an audio device
/// that refused to open from one that opened and never asked for a sample.
#[cfg(target_os = "android")]
macro_rules! note {
    ($($arg:tt)*) => { log::info!($($arg)*) };
}

/// Off Android there is no logcat, and the desktop tests read state directly.
#[cfg(not(target_os = "android"))]
macro_rules! note {
    ($($arg:tt)*) => {{ let _ = format_args!($($arg)*); }};
}

/// Route Rust logging into logcat under one tag.
///
/// Called from the constructor rather than a separate init the caller could
/// forget: a log that only works when someone remembered to switch it on is a
/// log that is off during the failure worth reading.
#[cfg(target_os = "android")]
fn install_logging() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("SonduitFfi"),
        );
    });
}

#[cfg(not(target_os = "android"))]
fn install_logging() {}

/// How long a receive blocks before checking whether it should stop.
///
/// Short enough that stopping feels immediate, long enough that an idle
/// session is not spinning.
const RECV_TIMEOUT: Duration = Duration::from_millis(250);

/// How long [`Bridge::accept_invite`] waits for the desktop's key offer.
///
/// The announcement has just been delivered and the desktop answers it
/// immediately, so this is a round trip and a little slack, not a user's
/// thinking time. It is bounded because it runs on the caller's thread: the
/// user has pointed a camera at a screen and is waiting for the app to say
/// something.
const KEY_OFFER_WAIT: Duration = Duration::from_secs(3);

/// How often the announcement is repeated while waiting for the key offer.
///
/// The announcement is one unicast datagram and losing it used to cost a
/// session that never started. Repeating it while nothing has come back costs a
/// system call per address per second and removes the flake; the desktop
/// authenticates every copy against the same nonce and code, so a repeat
/// pairs once.
const ANNOUNCE_REPEAT: Duration = Duration::from_millis(900);

/// Refusals between log lines, once the first has been reported.
///
/// About three seconds of a fully refused stream at 6 ms a packet.
const REFUSALS_PER_LINE: u64 = 500;

/// Nonces the discovery responder remembers, so it can answer a key offer.
///
/// The offer is tagged against the nonce of the probe it follows, and this
/// device does not know which of the probes it has just answered the offer
/// belongs to. Four is enough for the three probes one scan sends plus one
/// overlapping scan, and remembering a nonce is not remembering a secret:
/// every probe put its nonce on the wire in the clear.
const REMEMBERED_NONCES: usize = 4;

/// Packets between drift corrections.
///
/// At 6 ms packets this is four times a second, matching the rate the UI
/// samples telemetry at. Crystal drift is a physical constant of the two
/// devices and does not change from moment to moment, so correcting faster
/// would only chase jitter.
const PACKETS_PER_CORRECTION: u32 = 40;

/// Ceiling on packets moved from the jitter buffer to the audio queue per
/// packet received.
///
/// Above one so a queue that has fallen behind can catch up, and small enough
/// that catching up cannot starve the socket. The loop it bounds cannot be
/// written as "drain until empty": a jitter buffer conceals a gap rather than
/// reporting one, so it will always produce another packet if asked.
const DRAIN_PER_PACKET: usize = 3;

/// Packets the audio queue should hold.
///
/// The queue exists to cover the gap between two independent clocks: this
/// thread wakes when a datagram arrives, the callback wakes when the device
/// asks. A device here reported a 96-frame burst and a 4 ms buffer, so two
/// six-millisecond packets covers a callback and the jitter in getting to it,
/// and costs twelve milliseconds of latency to do it.
///
/// Everything above this waits in the jitter buffer instead, which is the part
/// that knows how to reorder, conceal and retarget. Audio held in the queue is
/// latency and nothing else.
const QUEUE_FLOOR_PACKETS: usize = 2;

/// How the hand-off from the jitter buffer into the audio queue is paced.
///
/// The rule itself lives in `sonduit-core` so it can be run against a
/// synthetic timeline: it is a feedback loop, and the one that used to be
/// written out here had no path back down. See `sonduit_core::pacing`.
const PACING: PacingConfig = PacingConfig {
    floor_packets: QUEUE_FLOOR_PACKETS,
    max_per_packet: DRAIN_PER_PACKET,
};

/// The master secret this device holds, and a counter that says when it moved.
///
/// One secret, because Sonduit has one sender at a time. It arrives from
/// whichever pairing path the user took -- the discovery responder answering a
/// key offer, or [`Bridge::accept_invite`] answering the one that follows a
/// scanned QR -- and it is read by the receive thread, which is on neither of
/// those paths.
///
/// The epoch is what lets the receive thread notice a pairing that happened
/// while it was already running without taking a lock on every datagram: it
/// reads one atomic per packet and touches the mutex only when the number has
/// changed. A pairing during a session is ordinary, because a session has to be
/// running before the QR path can announce a port at all.
struct Keys {
    /// Bumped on every change, including a clear.
    epoch: AtomicU64,
    /// `None` until this device has paired, and again after the user asks for
    /// a new code.
    secret: Mutex<Option<SessionSecret>>,
}

impl Keys {
    fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            secret: Mutex::new(None),
        }
    }

    /// Adopt the secret from a completed pairing, replacing any before it.
    ///
    /// Replacing rather than adding: the pairing that just happened is the one
    /// that is current, and a receiver holding two keys would accept audio
    /// from a desktop the user has since re-paired away from.
    fn adopt(&self, secret: SessionSecret) {
        if let Ok(mut held) = self.secret.lock() {
            *held = Some(secret);
            // Released after the store below, so a reader that sees the new
            // epoch cannot then read the old secret.
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Forget the pairing, returning this device to refusing sealed audio and
    /// accepting cleartext.
    fn clear(&self) {
        if let Ok(mut held) = self.secret.lock() {
            *held = None;
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The current epoch, for a reader deciding whether to look further.
    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// The secret as it stands, cloned so the lock is not held across use.
    fn secret(&self) -> Option<SessionSecret> {
        self.secret.lock().ok().and_then(|held| held.clone())
    }
}

/// Bridge lifecycle state, mirrored into the Android UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum BridgeState {
    /// Not connected to a sender.
    #[default]
    Idle,
    /// Listening, but no audio has arrived yet.
    Discovering,
    /// Receiving audio.
    Streaming,
    /// Stopped because of an error.
    Failed,
}

/// Errors surfaced across the FFI boundary.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// The bridge is not running.
    #[error("bridge is not running")]
    NotRunning,

    /// A session is already running.
    #[error("bridge is already running")]
    AlreadyRunning,

    /// Transport failure.
    #[error("transport error: {reason}")]
    Transport {
        /// What the socket layer reported.
        reason: String,
    },

    /// The audio device could not be opened.
    #[error("playback error: {reason}")]
    Playback {
        /// What AAudio reported.
        reason: String,
    },

    /// The scanned text was not a Sonduit pairing invite.
    #[error("that is not a Sonduit pairing code")]
    BadInvite,

    /// The announcement was delivered but the computer never agreed a key.
    ///
    /// Raised rather than swallowed because it is now a knowable outcome. The
    /// desktop answers a verified announcement with its half of the key
    /// agreement, so silence means the announcement did not arrive, arrived
    /// after the code expired, or was refused. Without a key this device
    /// cannot be sent audio at all, so reporting success here would be
    /// reporting a pairing that is not one.
    #[error("the computer did not finish pairing")]
    PairingIncomplete,

    /// No session key could be generated on this device.
    #[error("no session key could be generated: {reason}")]
    NoEntropy {
        /// What the platform's random source reported.
        reason: String,
    },
}

/// What the Android UI displays.
///
/// A flat record of plain scalars: UniFFI copies this across the boundary on
/// every poll, and anything richer would cost more than it tells the user.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct BridgeTelemetry {
    /// Lifecycle state.
    pub state: BridgeState,
    /// Datagrams accepted by the jitter buffer.
    pub packets_accepted: u64,
    /// Datagrams that arrived after their slot had been played.
    pub packets_late: u64,
    /// Datagrams that never arrived.
    pub packets_lost: u64,
    /// Datagrams rejected before decoding.
    pub packets_malformed: u64,
    /// Current buffer depth, in milliseconds.
    pub buffer_depth_ms: f64,
    /// Depth the buffer is aiming for, in milliseconds.
    pub buffer_target_ms: f64,
    /// RFC 3550 inter-arrival jitter estimate, in milliseconds.
    pub jitter_ms: f64,
    /// Frames of silence emitted to cover lost packets.
    pub concealed_frames: u64,
    /// Sample rate the sender is producing, zero before the first packet.
    pub sample_rate: u32,
    /// Channel count the sender is producing.
    pub channels: u8,
    /// Why the audio device could not be opened, if it could not.
    ///
    /// Carried in telemetry rather than raised from `start`, because the
    /// device is opened by the receive thread after the first packet and there
    /// is no call left to fail.
    pub playback_error: Option<String>,
    /// Measured clock difference, positive when the sender runs fast.
    ///
    /// `None` until the estimator has enough observations, which takes about
    /// 25 seconds. Reporting a number before then would be reporting noise.
    pub drift_ppm: Option<f64>,
    /// Correction currently being applied, in parts per million.
    pub correction_ppm: f64,
    /// Datagrams refused by the cipher layer.
    ///
    /// Sealed audio that did not authenticate, sealed audio arriving with no
    /// key to open it, and -- the case that matters most -- cleartext audio
    /// arriving at a receiver that holds a key. That last one is the downgrade
    /// defence: a keyed receiver that still played version 1 packets would let
    /// an attacker simply send version 1, and the encryption would be
    /// decoration. Every one of them is counted here and none of them is
    /// played.
    ///
    /// Separate from `packets_malformed`, which is a datagram that is not a
    /// packet at all. A refusal is a well-formed packet this receiver is not
    /// allowed to accept, which is a different thing to see on a screen.
    pub packets_refused: u64,
    /// Whether the audio being played is encrypted.
    ///
    /// True once this device holds a master secret, which is exactly when it
    /// will accept sealed audio and refuse anything else. Shown because a
    /// session that is not encrypted must never look like one that is.
    pub encrypted: bool,
    /// Which link the audio is arriving over. Empty before the first packet.
    ///
    /// Taken from the wired-link flag the sender sets in the packet header,
    /// because the sender is the only end that knows which interface it chose.
    /// A sender too old to set the flag is not claiming Wi-Fi, only declining
    /// to say, and only that case falls back to a guess from the source
    /// address.
    pub transport: String,
}

/// State shared between the receive thread and the FFI callers.
struct Shared {
    source: Mutex<JitterSource>,
    state: Mutex<BridgeState>,
    malformed: Mutex<u64>,
    /// Datagrams the cipher layer refused. See [`BridgeTelemetry::packets_refused`].
    refused: Mutex<u64>,
    /// The pairing this session is keyed from, shared with the pairing paths
    /// so that a pairing completed mid-session takes effect on the next packet.
    keys: Arc<Keys>,
    format: Mutex<Option<Format>>,
    /// The producer half of the handoff to the audio callback.
    ///
    /// Written only by the receive thread. The callback holds the other half
    /// and takes no lock at all. A mutex shared with the callback is what
    /// produced 1536 ms of latency with crackle on the first real device.
    queue: Mutex<Option<sonduit_core::handoff::Producer>>,
    /// Opened by the receive thread once the format is known, and stopped by
    /// [`Bridge::stop`]. `None` on any platform without AAudio, and before the
    /// first packet arrives on Android.
    #[cfg(target_os = "android")]
    playback: Mutex<Option<sonduit_playback_android::Playback>>,
    /// Why playback could not be opened, surfaced through telemetry.
    playback_error: Mutex<Option<String>>,
    /// Drift as last measured, and the correction last applied.
    ///
    /// Written by the receive thread, read by whoever asks for telemetry.
    drift: Mutex<(Option<f64>, f64)>,
    /// The link the buffer was sized for.
    transport: Mutex<Option<Transport>>,
}

/// A handle the Android app holds for as long as it is running.
///
/// Not for as long as a session: answering discovery probes is what makes this
/// device findable at all, and it has to work while the app sits idle showing
/// its pairing code. That responder is created here and lives here, so a
/// session can start and stop underneath it without rebinding anything.
#[derive(uniffi::Object)]
pub struct Bridge {
    inner: Mutex<Option<Running>>,
    shared: Mutex<Option<Arc<Shared>>>,
    /// The name announced in reply to probes.
    ///
    /// Shared with the responder rather than copied into it, for the same
    /// reason as the code below: the responder outlives every session, and a
    /// name captured when it started would still be the placeholder long after
    /// the app had learned the real one.
    device_name: Arc<Mutex<String>>,
    /// The code this device will answer probes with.
    ///
    /// Generated once per process rather than per session, so a user who stops
    /// and starts the bridge does not have to retype it on the desktop.
    ///
    /// Shared with the announce thread rather than copied into it. A copy
    /// would go stale the moment the code changed, and the phone would then
    /// show one code on screen while proving it knew another.
    pairing: Arc<Mutex<PairingCode>>,
    /// The master secret, shared with everything that agrees or uses one.
    ///
    /// On the [`Bridge`] rather than on a session, for the same reason the
    /// pairing code is: pairing happens while the app is open, sessions come
    /// and go underneath it, and a secret captured into a session would be
    /// lost the first time the user stopped and started one.
    keys: Arc<Keys>,
    /// The port an announcement advertises, which is where the desktop will
    /// send audio.
    ///
    /// While a session is running this is the port that session actually
    /// bound. While none is, it is the port the next one will bind, which is
    /// the only honest answer available: being found is what prompts the user
    /// to start it.
    advertised_port: Arc<AtomicU16>,
    /// The discovery responder, alive for as long as this handle is.
    ///
    /// `None` when the discovery port could not be bound, which in practice
    /// means another copy of this app already holds it. Nothing else in this
    /// crate binds that port, so there is one owner of it per process.
    ///
    /// Held for its `Drop` and never otherwise read: dropping it is what stops
    /// the thread and gives the port back.
    #[allow(dead_code)]
    responder: Option<Responder>,
}

struct Running {
    stop: Arc<AtomicBool>,
    receive: Option<std::thread::JoinHandle<()>>,
    /// The port audio is arriving on, which is what an announcement has to
    /// advertise. Kept here because only a running session has one, and
    /// [`Bridge::accept_invite`] refuses without it.
    port: u16,
}

/// The thread that answers discovery probes, and the socket it owns.
///
/// Its lifetime is a [`Bridge`], not a session. That is the whole point:
/// spawning it from `start` meant a phone displaying its six digits with
/// nothing started answered no probe at all, so the desktop's typed-code scan
/// reported there was no device on the network.
///
/// One socket, one owner. Two things bound to the well-known discovery port is
/// the "only one usage of each socket address" failure, and the way to not have
/// it is for the port to be claimed in exactly one place.
struct Responder {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Responder {
    /// Bind the discovery port and start answering.
    ///
    /// The socket is bound here rather than on the new thread, so a port
    /// already in use is reported to the caller. Discovering it on the thread
    /// would turn it into an immediate exit and a device that is silently
    /// unfindable, which is the failure this exists to prevent.
    ///
    /// # Errors
    /// Whatever binding the port or spawning the thread reported.
    fn spawn(
        discovery_port: u16,
        name: Arc<Mutex<String>>,
        audio_port: Arc<AtomicU16>,
        code: Arc<Mutex<PairingCode>>,
        keys: Arc<Keys>,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, discovery_port)))?;
        let _ = socket.set_broadcast(true);
        socket.set_read_timeout(Some(RECV_TIMEOUT))?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("sonduit-announce".into())
                .spawn(move || announce_loop(&socket, &stop, &name, &audio_port, &code, &keys))?
        };

        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Responder {
    /// Stop answering, and do not return until the thread has gone.
    ///
    /// Joined rather than detached: a detached thread keeps the discovery port
    /// for as long as it likes, and the next `Bridge` in the same process
    /// would fail to bind it. The wait is bounded by [`RECV_TIMEOUT`].
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Bridge {
    /// Build a bridge whose responder answers on `discovery_port`.
    ///
    /// Separate from [`Bridge::new`] only so the tests can take a port of
    /// their own: the real one is a fixed well-known number, and a test that
    /// claimed it would collide with a desktop pairing dialog on the same
    /// machine, or with another test.
    ///
    /// A responder that cannot bind is recorded as absent rather than raised.
    /// There is no caller to raise it to, this being a constructor the UI runs
    /// before it has anywhere to show an error, and a bridge that cannot be
    /// discovered can still be paired by QR and can still play audio.
    /// Wait for the desktop's key offer on `socket` and answer it.
    ///
    /// Bounded by [`KEY_OFFER_WAIT`]. Anything that does not verify against
    /// the invite's own nonce and code is ignored rather than refused out
    /// loud: this socket is on an open network and a stray datagram from
    /// somebody else must not be able to end the pairing.
    fn answer_key_offer(
        &self,
        socket: &UdpSocket,
        invite: &Invite,
        mut announce: impl FnMut(),
    ) -> Result<SessionSecret, FfiError> {
        let seed = entropy::key_seed().map_err(|error| FfiError::NoEntropy {
            reason: error.to_string(),
        })?;

        socket
            .set_read_timeout(Some(RECV_TIMEOUT))
            .map_err(|error| FfiError::Transport {
                reason: error.to_string(),
            })?;

        let deadline = std::time::Instant::now() + KEY_OFFER_WAIT;
        let mut datagram = [0_u8; 256];
        let mut sent_at = std::time::Instant::now();
        // One nonce here, the invite's, and one offer answered before this
        // returns. The responder is still the type that does the answering,
        // so this path cannot drift away from the discovery one: the desktop
        // repeats its offer on both, and both have to answer a repeat the
        // same way.
        let mut responder = handshake::Responder::new();

        while std::time::Instant::now() < deadline {
            // The announcement goes again while nothing has come back. One
            // unicast datagram lost on a radio used to cost the user a session
            // that never started for no visible reason; now it would cost them
            // a refused pairing, which is better and is still avoidable. The
            // desktop authenticates every copy against the same nonce and code,
            // so a repeat is not a second pairing.
            if sent_at.elapsed() >= ANNOUNCE_REPEAT {
                sent_at = std::time::Instant::now();
                announce();
            }

            let Ok((length, from)) = socket.recv_from(&mut datagram) else {
                continue;
            };
            let Some(answered) =
                responder.answer(&datagram[..length], &[invite.nonce], &invite.code, seed)
            else {
                continue;
            };
            let _ = socket.send_to(&answered.accept, from);
            // `None` cannot happen on the first answer, and returning here on
            // a repeat would hand back a secret this end never derived.
            if let Some(secret) = answered.secret {
                return Ok(secret);
            }
        }

        Err(FfiError::PairingIncomplete)
    }

    fn with_discovery_port(discovery_port: u16) -> Self {
        install_logging();

        let device_name = Arc::new(Mutex::new("Sonduit".to_string()));
        let pairing = Arc::new(Mutex::new(PairingCode::from_seed(random_seed())));
        let advertised_port = Arc::new(AtomicU16::new(DEFAULT_PORT));
        let keys = Arc::new(Keys::new());

        let responder = match Responder::spawn(
            discovery_port,
            Arc::clone(&device_name),
            Arc::clone(&advertised_port),
            Arc::clone(&pairing),
            Arc::clone(&keys),
        ) {
            Ok(responder) => Some(responder),
            Err(error) => {
                note!("discovery responder could not bind port {discovery_port}: {error}");
                None
            }
        };

        Self {
            inner: Mutex::new(None),
            shared: Mutex::new(None),
            device_name,
            pairing,
            keys,
            advertised_port,
            responder,
        }
    }
}

#[uniffi::export]
impl Bridge {
    /// Create an idle bridge that is already answering discovery probes.
    ///
    /// Idle means no session, not silent. Constructing this claims the
    /// discovery port and starts the responder, because the moment worth being
    /// findable in is the one where the user is reading the pairing code off
    /// the screen and has pressed nothing.
    #[uniffi::constructor]
    #[must_use]
    pub fn new() -> Self {
        Self::with_discovery_port(discovery::DISCOVERY_PORT)
    }

    /// The pairing code to show the user.
    ///
    /// The desktop will not accept this device's announcement without it, so
    /// it has to be on screen before a scan is useful.
    #[must_use]
    pub fn pairing_code(&self) -> String {
        self.pairing
            .lock()
            .map(|code| code.to_display())
            .unwrap_or_default()
    }

    /// Generate a new pairing code, and forget the key that went with it.
    ///
    /// For a user who believes the old one has been seen by someone else. Any
    /// desktop paired with the previous code stops being able to find this
    /// device, which is the point.
    ///
    /// The master secret goes with it, and that is the larger change: it was
    /// agreed under the code being replaced, so keeping it would leave this
    /// device able to play audio from a pairing the user has just revoked.
    ///
    /// This is also the one way back to an unkeyed receiver, which is what a
    /// user needs to receive from an unmodified Scream sender: that sender has
    /// no key and cannot have one, and a keyed receiver refuses it. The trade
    /// is stated rather than hidden -- ADR-009 makes a pairing worth keeping,
    /// and this is the button that throws one away.
    pub fn regenerate_pairing_code(&self) {
        if let Ok(mut code) = self.pairing.lock() {
            *code = PairingCode::from_seed(random_seed());
        }
        self.keys.clear();
        note!("pairing code regenerated; the previous session key is discarded");
    }

    /// Whether this device holds a pairing key, and so plays encrypted audio.
    ///
    /// Read by the UI before a session exists, which is why it is not only a
    /// telemetry field: the screen showing the pairing code is the screen that
    /// should say whether this device is paired.
    #[must_use]
    pub fn is_paired(&self) -> bool {
        self.keys.secret().is_some()
    }

    /// Pair from a QR code the desktop displayed.
    ///
    /// The desktop cannot be found by broadcast when it sits in another
    /// subnet, so it puts its own addresses, the discovery port, a fresh nonce
    /// and a pairing code on screen and this device reads them with the
    /// camera. The announcement then goes out by unicast to each of those
    /// addresses, which is what crosses a router.
    ///
    /// The datagram is exactly the reply
    /// [`sonduit_transport::discovery::encode_announce`] builds for a
    /// broadcast probe, tagged with the same HMAC keyed by the same pairing
    /// code. Nothing new is trusted on either end, so the only part of the
    /// threat model that changes is that the code now sits on the desktop's
    /// screen: somebody who photographs it from across the room learns it and
    /// could announce a device of their own in this one's place. That was
    /// already true of the code this app displays for the user to read aloud.
    /// The code itself still never reaches the wire.
    ///
    /// The desktop learns this device's address from the datagram's source,
    /// so no address of ours is in the payload and none can be spoofed there.
    ///
    /// A session must already be running: the announcement advertises the port
    /// audio will arrive on, and until the socket is bound there is no such
    /// port to advertise.
    ///
    /// # What happens after the announcement
    ///
    /// The desktop answers a verified announcement with its half of an
    /// ephemeral Diffie-Hellman, and this device answers that with its own and
    /// keeps the master secret. Until that has happened the pairing is not
    /// finished and no audio can flow: the desktop will not send to a device
    /// it holds no key for, and this device would refuse cleartext anyway.
    ///
    /// So returning `Ok` now means the pairing completed, which is more than
    /// it used to mean. Silence is [`FfiError::PairingIncomplete`] rather than
    /// a success the user only discovers was not one when no sound arrives.
    ///
    /// # Errors
    /// Returns [`FfiError::BadInvite`] when the scanned text is not a Sonduit
    /// invite, [`FfiError::NotRunning`] when no session is listening,
    /// [`FfiError::Transport`] when no address in the invite could be reached
    /// at all, [`FfiError::NoEntropy`] when no key pair can be generated, and
    /// [`FfiError::PairingIncomplete`] when the computer never answered.
    pub fn accept_invite(&self, payload: String) -> Result<(), FfiError> {
        let invite = Invite::parse(&payload).ok_or(FfiError::BadInvite)?;

        let audio_port = {
            let running = self.inner.lock().map_err(|_| FfiError::NotRunning)?;
            running.as_ref().ok_or(FfiError::NotRunning)?.port
        };

        // The desktop chose this code, so this device adopts it rather than
        // keeping its own. Otherwise a later broadcast scan from the same
        // desktop would be answered with a code it has never seen.
        {
            let mut code = self.pairing.lock().map_err(|_| FfiError::NotRunning)?;
            *code = invite.code.clone();
        }

        let name = self
            .device_name
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| "Sonduit".to_string());

        let socket =
            UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).map_err(|error| {
                FfiError::Transport {
                    reason: error.to_string(),
                }
            })?;
        let datagram = discovery::encode_announce(&name, audio_port, &invite.nonce, &invite.code);

        // Every address is tried, not just the first. The desktop cannot tell
        // which of its interfaces this phone shares, so it offers all of them;
        // the ones on other links fail here and cost a system call each.
        let announce = |socket: &UdpSocket| {
            let mut delivered = false;
            for address in &invite.addresses {
                if socket
                    .send_to(&datagram, SocketAddr::from((*address, invite.port)))
                    .is_ok()
                {
                    delivered = true;
                }
            }
            delivered
        };

        if !announce(&socket) {
            return Err(FfiError::Transport {
                reason: "no address in the pairing code could be reached".to_string(),
            });
        }

        // The desktop's key offer comes back to the source address of the
        // announcement, so it arrives on this same socket. Waiting for it here
        // rather than on a background thread keeps the whole pairing inside
        // the one call the UI made, and lets the failure be reported as a
        // failure.
        let secret = self.answer_key_offer(&socket, &invite, || {
            announce(&socket);
        })?;
        self.keys.adopt(secret);
        note!("paired by QR; audio from this computer will be encrypted");
        Ok(())
    }

    /// Set the name announced in reply to discovery probes.
    pub fn set_device_name(&self, name: String) {
        if let Ok(mut current) = self.device_name.lock() {
            *current = name;
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn state(&self) -> BridgeState {
        self.shared
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|shared| read_state(shared)))
            .unwrap_or_default()
    }

    /// Most recent telemetry snapshot.
    #[must_use]
    pub fn telemetry(&self) -> BridgeTelemetry {
        let Ok(guard) = self.shared.lock() else {
            return BridgeTelemetry::default();
        };
        let Some(shared) = guard.as_ref() else {
            return BridgeTelemetry::default();
        };

        let state = read_state(shared);
        let malformed = shared.malformed.lock().map(|count| *count).unwrap_or(0);
        let refused = shared.refused.lock().map(|count| *count).unwrap_or(0);
        let encrypted = shared.keys.secret().is_some();
        let format = shared.format.lock().ok().and_then(|value| *value);
        let playback_error = shared
            .playback_error
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let (drift_ppm, correction_ppm) = shared
            .drift
            .lock()
            .map(|value| *value)
            .unwrap_or((None, 0.0));
        let transport = shared
            .transport
            .lock()
            .ok()
            .and_then(|value| *value)
            .map(|link| match link {
                Transport::Usb => "usb",
                Transport::WiFi => "wifi",
            })
            .unwrap_or_default()
            .to_string();

        // try_lock: telemetry is a nice-to-have, and blocking here would put
        // the UI thread behind the audio callback.
        let Ok(source) = shared.source.try_lock() else {
            return BridgeTelemetry {
                state,
                packets_malformed: malformed,
                packets_refused: refused,
                encrypted,
                sample_rate: format.map_or(0, |f| f.sample_rate),
                channels: format.map_or(0, |f| f.channels),
                playback_error,
                drift_ppm,
                correction_ppm,
                transport,
                ..BridgeTelemetry::default()
            };
        };

        let stats = source.buffer().stats();
        BridgeTelemetry {
            state,
            packets_accepted: stats.accepted,
            packets_late: stats.too_late,
            packets_lost: stats.lost,
            packets_malformed: malformed,
            packets_refused: refused,
            encrypted,
            buffer_depth_ms: source.buffer().depth_ms(),
            buffer_target_ms: source.buffer().target_ms(),
            jitter_ms: source.buffer().jitter_ms(),
            concealed_frames: source.concealed_frames(),
            sample_rate: format.map_or(0, |f| f.sample_rate),
            channels: format.map_or(0, |f| f.channels),
            playback_error,
            drift_ppm,
            correction_ppm,
            transport,
        }
    }

    /// Begin listening for a sender.
    ///
    /// `port` of zero means the default.
    ///
    /// The socket is bound here, so a port already in use is reported to the
    /// caller. The audio device is not: it is opened by the receive thread
    /// once a packet has said what format to open it in. Guessing would mean
    /// either resampling everything or reopening the device a moment after
    /// starting, and a device opened at the wrong rate is the single largest
    /// avoidable source of latency on this path.
    ///
    /// Discovery is untouched. The responder is bound for the life of this
    /// handle and keeps answering across every start and stop; all this does
    /// is tell it which port to name.
    ///
    /// # Errors
    /// Returns [`FfiError::AlreadyRunning`] if a session is live, and
    /// [`FfiError::Transport`] when the socket cannot be bound.
    pub fn start(&self, port: u16) -> Result<(), FfiError> {
        let mut running = self.inner.lock().map_err(|_| FfiError::NotRunning)?;
        if running.is_some() {
            return Err(FfiError::AlreadyRunning);
        }

        let port = if port == 0 { DEFAULT_PORT } else { port };
        let socket =
            UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))).map_err(|error| {
                FfiError::Transport {
                    reason: error.to_string(),
                }
            })?;
        socket
            .set_read_timeout(Some(RECV_TIMEOUT))
            .map_err(|error| FfiError::Transport {
                reason: error.to_string(),
            })?;
        // Senders may multicast rather than address the phone directly, and a
        // receiver that has not joined the group never sees those datagrams.
        let _ = socket.join_multicast_v4(
            &sonduit_transport::DEFAULT_MULTICAST_GROUP.into(),
            &Ipv4Addr::UNSPECIFIED,
        );

        // The buffer is built on the common case so it exists before the first
        // packet; the receive thread rebuilds it if the sender disagrees.
        let format = Format::stereo_48k();
        let shared = Arc::new(Shared {
            source: Mutex::new(JitterSource::new(
                JitterBuffer::new(format, JitterConfig::default()),
                format,
            )),
            state: Mutex::new(BridgeState::Discovering),
            malformed: Mutex::new(0),
            refused: Mutex::new(0),
            keys: Arc::clone(&self.keys),
            format: Mutex::new(None),
            queue: Mutex::new(None),
            #[cfg(target_os = "android")]
            playback: Mutex::new(None),
            playback_error: Mutex::new(None),
            drift: Mutex::new((None, 0.0)),
            transport: Mutex::new(None),
        });

        let stop = Arc::new(AtomicBool::new(false));

        let receive = {
            let stop = Arc::clone(&stop);
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("sonduit-receive".into())
                .spawn(move || receive_loop(&socket, &stop, &shared))
                .map_err(|error| FfiError::Transport {
                    reason: error.to_string(),
                })?
        };

        // No responder is started here. One has been answering since this
        // handle was constructed, and all a session changes about it is the
        // port it names.
        self.advertised_port.store(port, Ordering::Relaxed);

        *running = Some(Running {
            stop,
            receive: Some(receive),
            port,
        });

        if let Ok(mut current) = self.shared.lock() {
            *current = Some(shared);
        }

        Ok(())
    }

    /// Stop the session and release the audio device.
    ///
    /// Discovery is deliberately not stopped: this device stays findable until
    /// the app itself goes away, which is when [`Responder`] is dropped.
    ///
    /// Idempotent, because the Android lifecycle calls it from more than one
    /// place and a service torn down twice is normal.
    ///
    /// # Errors
    /// Returns [`FfiError::NotRunning`] only if the internal lock is poisoned.
    pub fn stop(&self) -> Result<(), FfiError> {
        let mut running = self.inner.lock().map_err(|_| FfiError::NotRunning)?;
        let Some(mut session) = running.take() else {
            return Ok(());
        };

        session.stop.store(true, Ordering::Relaxed);

        // The device is released before the threads are joined: a stopped
        // stream stops calling back, so the receive thread is not racing an
        // audio callback while it winds down.
        #[cfg(target_os = "android")]
        if let Ok(shared) = self.shared.lock() {
            if let Some(shared) = shared.as_ref() {
                if let Ok(mut playback) = shared.playback.lock() {
                    if let Some(playback) = playback.take() {
                        let _ = playback.stop();
                    }
                }
            }
        }

        if let Some(handle) = session.receive.take() {
            let _ = handle.join();
        }

        // Back to the port the next session will bind. The responder keeps
        // running either way: this device is still on the network and still
        // worth finding, and the user who stops a session is often the user
        // about to pair a different computer.
        self.advertised_port.store(DEFAULT_PORT, Ordering::Relaxed);

        if let Ok(mut shared) = self.shared.lock() {
            if let Some(shared) = shared.as_ref() {
                set_state(shared, BridgeState::Idle);
            }
            *shared = None;
        }

        Ok(())
    }
}

fn read_state(shared: &Shared) -> BridgeState {
    shared
        .state
        .lock()
        .map(|guard| *guard)
        .unwrap_or(BridgeState::Failed)
}

/// Count a datagram the cipher layer would not accept.
///
/// Separate from the malformed count on purpose. Malformed is "that was not a
/// packet"; this is "that was a packet and this receiver is not allowed to
/// play it", which is what the user needs to see when a sender is using the
/// wrong key or somebody is injecting audio.
fn refuse(shared: &Shared, since_report: &mut u64) {
    *since_report += 1;
    if let Ok(mut count) = shared.refused.lock() {
        *count += 1;
    }

    // Logged here rather than only on the periodic tick, because that tick
    // runs on the packets that were accepted: a receiver refusing every
    // datagram -- a sender using the wrong key, which is the case somebody
    // would actually need to diagnose -- would never reach it and logcat
    // would say nothing at all.
    //
    // The first one and then rarely, so a stream that is entirely refused
    // costs one line and then a line every few seconds instead of one every
    // six milliseconds.
    if *since_report == 1 || *since_report % REFUSALS_PER_LINE == 0 {
        note!("refused a datagram the session key does not accept ({since_report} so far)");
    }
}

fn set_state(shared: &Shared, state: BridgeState) {
    if let Ok(mut current) = shared.state.lock() {
        *current = state;
    }
}

/// Decode arriving datagrams into the jitter buffer.
fn receive_loop(socket: &UdpSocket, stop: &AtomicBool, shared: &Arc<Shared>) {
    // Arrival times come from a monotonic clock started with the thread. They
    // are only ever differenced, so the epoch does not matter; what matters is
    // that it never steps backwards, which wall-clock time does every time the
    // phone syncs its clock.
    let start = std::time::Instant::now();
    let mut datagram = [0_u8; sonduit_transport::MAX_DATAGRAM_BYTES];

    // The sender has no other way to learn anything. Without these reports it
    // can only describe its own socket, and it showed a working session with
    // no device on the network at all.
    let mut report_buffer = [0_u8; SEALED_FEEDBACK_BYTES];
    let mut last_report = std::time::Instant::now();
    let mut last_accepted: Option<(u32, std::time::Instant)> = None;
    let mut seen_audio = false;

    // Drift correction. The estimator measures, the controller decides, the
    // resampler applies. All three are rebuilt when the format changes, since
    // none of what they learned about the old stream applies to the new one.
    let mut estimator: Option<DriftEstimator> = None;
    let mut controller = RatioController::new(RatioConfig::default());
    let mut resampler: Option<DriftResampler> = None;
    // Frames the sender has produced, accumulated across sequence wraps. The
    // packet timestamp is 32 bits and wraps in about 25 hours, which is well
    // inside a plausible session.
    let mut sender_frames = 0_u64;
    let mut previous_timestamp: Option<u32> = None;
    let mut since_correction = 0_u32;
    // Packets shed since the last report. Accumulated rather than logged where
    // it happens: the bound is checked on every arrival, and a device that has
    // stopped calling back would otherwise produce a line every six
    // milliseconds about the one thing that is already obvious.
    let mut shed_packets = 0_u64;
    // Packets shed at the jitter buffer's hard packet ceiling since the last
    // report, which is a different fact from the line above and reported
    // separately. The bound on the sum is what is supposed to keep the depth
    // sane; the ceiling is the floor underneath it, and reaching it says the
    // bound never ran -- there was no playback queue to measure against, or
    // the lock guarding it was not available -- rather than that the link is
    // holding too much.
    let mut ceiling_packets = 0_u64;
    // Which link the audio is arriving over, as the sender declares it in
    // every packet header. Wi-Fi until the first packet says otherwise,
    // because holding too much audio is recoverable and holding too little is
    // heard immediately. A change mid-session is confirmed over several
    // packets before it is acted on; see `LinkWatch`.
    let mut link = LinkWatch::new(Transport::WiFi);
    // Reused so draining allocates nothing per packet.
    let mut staging: Vec<u8> = Vec::with_capacity(sonduit_core::format::PCM_PAYLOAD_BYTES);
    // Scream carries no sequence number, so one is synthesised on arrival.
    // Reordering cannot be repaired for that wire format, which is a property
    // of the protocol and not of this code.
    let mut scream_sequence = 0_u16;

    // The cipher state, rebuilt whenever the pairing changes underneath this
    // thread. `u64::MAX` rather than zero so the first packet always syncs:
    // a device that paired before the session started must not spend its first
    // packets refusing audio it holds the key for.
    let mut key_epoch = u64::MAX;
    let mut opener: Option<Opener> = None;
    let mut feedback_sealer: Option<FeedbackSealer> = None;
    // Reused, so opening allocates nothing per packet. Sized for the largest
    // datagram this transport will accept, which bounds the plaintext inside
    // one.
    let mut opened = vec![0_u8; sonduit_transport::MAX_DATAGRAM_BYTES];
    // Refusals since the last log line. Counted rather than logged where they
    // happen: a sender pointed at this device with the wrong key would produce
    // a line every six milliseconds about one fact.
    let mut refused_since_report = 0_u64;

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        let arrival = start.elapsed().as_nanos() as u64;
        let bytes = &datagram[..length];

        // One relaxed atomic read per datagram, and the mutex only when a
        // pairing has actually happened. Pairing during a session is the
        // ordinary case on the QR path, where a session has to be running
        // before there is a port to announce.
        let epoch = shared.keys.epoch();
        if epoch != key_epoch {
            key_epoch = epoch;
            let secret = shared.keys.secret();
            opener = secret.clone().map(Opener::new);
            feedback_sealer = secret.as_ref().and_then(|secret| {
                match entropy::stream_salt() {
                    Ok(salt) => Some(FeedbackSealer::new(secret, salt)),
                    // No salt means no sealed reports. The sender then sees a
                    // receiver that never answers, which is honest: what it
                    // must not see is a report it cannot authenticate, and
                    // what it must never get is a cleartext one from a keyed
                    // session.
                    Err(error) => {
                        note!("no random salt for the feedback key, reports are off: {error}");
                        None
                    }
                }
            });
            note!(
                "session key {}",
                if opener.is_some() {
                    "adopted: audio must be sealed"
                } else {
                    "cleared: this device is unpaired"
                }
            );
        }

        // Which wire this datagram is, and whether this receiver is allowed to
        // accept it at all. There is deliberately no arm that plays a packet
        // the key says no to: a receiver that fell back to cleartext when
        // opening failed would turn the encryption into a suggestion, and a
        // receiver that played ciphertext as PCM would put a full-scale noise
        // burst into somebody's headphones.
        let version = sonduit_transport::sonduit_version(bytes);
        let sealed_version = Some(sonduit_core::packet::SONDUIT_VERSION_SEALED);

        let decoded = if opener.is_some() && version != sealed_version {
            // The downgrade defence, and it is not optional. Cleartext Sonduit,
            // Scream and anything else all land here: a keyed receiver that
            // still accepted any of them would let an attacker pick the format
            // with no key in it.
            refuse(shared, &mut refused_since_report);
            continue;
        } else if version == sealed_version {
            let Some(opener) = opener.as_mut() else {
                // Sealed audio and no key. Nothing to try: the packet is not
                // for this pairing and cannot be made into audio.
                refuse(shared, &mut refused_since_report);
                continue;
            };
            match opener.open(bytes, &mut opened) {
                Ok(packet) => Some((
                    packet.format,
                    packet.sequence,
                    packet.timestamp_frames,
                    // The one copy the cleartext path makes as well: the
                    // buffer below owns its audio. Opening itself wrote into
                    // the reused buffer above and allocated nothing.
                    packet.pcm.to_vec(),
                    packet.wired_link(),
                )),
                // Forged, corrupted, replayed or sent under another key. The
                // four are not told apart because none of them is audio this
                // receiver may play.
                Err(_) => {
                    refuse(shared, &mut refused_since_report);
                    continue;
                }
            }
        } else {
            match classify(bytes) {
                Some(Wire::Sonduit) => SonduitPacket::decode(bytes).ok().map(|packet| {
                    (
                        packet.format,
                        packet.sequence,
                        packet.timestamp_frames,
                        packet.pcm.to_vec(),
                        packet.wired_link(),
                    )
                }),
                // Scream's header has no room to say, so it never claims a wired
                // link and the address is all there is to go on.
                Some(Wire::Scream) => ScreamPacket::decode(bytes).ok().map(|packet| {
                    let sequence = scream_sequence;
                    scream_sequence = scream_sequence.wrapping_add(1);
                    let frames = (packet.pcm.len() / packet.format.bytes_per_frame()) as u32;
                    (
                        packet.format,
                        sequence,
                        u32::from(sequence).wrapping_mul(frames),
                        packet.pcm.to_vec(),
                        false,
                    )
                }),
                None => None,
            }
        };

        let Some((format, sequence, timestamp, pcm, wired_link)) = decoded else {
            if let Ok(mut count) = shared.malformed.lock() {
                *count += 1;
            }
            continue;
        };

        // The first packet decides the format. A sender that changes format
        // mid-session is a new session as far as the buffer is concerned:
        // keeping the old one would play the new audio at the wrong rate.
        let changed = {
            let mut current = match shared.format.lock() {
                Ok(guard) => guard,
                Err(_) => continue,
            };
            if *current != Some(format) {
                *current = Some(format);
                true
            } else {
                false
            }
        };

        if changed {
            // Everything learned about the previous stream is about a
            // different clock pair and a different rate.
            // A new stream, so the link it declares is taken at once: there
            // is no buffer state to protect and nothing yet to confirm it
            // against.
            link = LinkWatch::new(link_of(from, wired_link));
            if let Ok(mut slot) = shared.transport.lock() {
                *slot = Some(link.link());
            }
            estimator = Some(DriftEstimator::new(DriftConfig::for_rate(
                format.sample_rate,
            )));
            controller.reset();
            resampler = DriftResampler::new(format, pcm.len() / format.bytes_per_frame()).ok();
            sender_frames = 0;
            previous_timestamp = None;
            since_correction = 0;
        }

        // The link can change under a session that never stops. The desktop
        // migrates one between Wi-Fi and a USB tether without interrupting the
        // stream and re-declares the new link in every packet after it, so the
        // format is unchanged and none of the block above runs. Until this
        // existed nothing else did either: the buffer kept the policy it was
        // built with. Coming off USB that meant holding the wired floor of
        // 6 ms against a radio's jitter for the nine seconds the adaptation
        // needed to grow out of it, which is a plausible underrun in exactly
        // the seconds after the user pulled a cable and is listening for one.
        //
        // Confirmed over several packets rather than taken from one. Nothing
        // authenticates this wire, and reacting thirty milliseconds late is
        // free next to reacting to a single corrupted header.
        if !changed {
            if let Some(migrated) = link.observe(link_of(from, wired_link)) {
                note!("link migrated to {migrated:?}; retuning in place");
                if let Ok(mut slot) = shared.transport.lock() {
                    *slot = Some(migrated);
                }

                // Swapped in place, and deliberately not a new JitterSource.
                // A new one discards the audio the old one is holding, which
                // is the gap the migration was built to avoid; `open_playback`
                // is not called for the same reason, and stays gated on a
                // format change alone, because reopening AAudio costs audible
                // silence and the format has not changed.
                if let Ok(mut source) = shared.source.lock() {
                    source
                        .buffer_mut()
                        .retune(JitterConfig::for_transport(migrated));
                }

                // The arrival timeline genuinely stepped: the same audio now
                // crosses a different path with a different transit time, and
                // a line fitted across that step measures the step and calls
                // it drift. This is the same trio the estimator's own gap
                // reset drives below, for the same reason.
                //
                // Deliberately not `sender_frames` and not
                // `previous_timestamp`. The sender's clock did not step and
                // its packets are continuous; those two are the sender's side
                // of the comparison, and resetting them would corrupt the very
                // quantity drift is measured against.
                if let Some(estimator) = estimator.as_mut() {
                    estimator.reset();
                }
                controller.reset();
                if let Some(resampler) = resampler.as_mut() {
                    resampler.reset();
                }
                if let Ok(mut slot) = shared.drift.lock() {
                    *slot = (None, 0.0);
                }
            }
        }

        // The sender's own frame count, which is the clock being compared
        // against the receiver's monotonic time. Differencing wrapped
        // timestamps rather than using them directly is what survives the wrap.
        let step =
            previous_timestamp.map_or(0, |previous| u64::from(timestamp.wrapping_sub(previous)));
        sender_frames += step;
        previous_timestamp = Some(timestamp);

        if let Some(estimator) = estimator.as_mut() {
            let resets_before = estimator.resets();
            estimator.observe(sender_frames, arrival);

            // The estimator throws its history away after a long gap: the
            // phone slept, or the route changed. The correction derived from
            // that history has to go with it, or the controller spends the
            // next minute unwinding a number that described a session that is
            // over.
            if estimator.resets() != resets_before {
                controller.reset();
                if let Some(resampler) = resampler.as_mut() {
                    resampler.reset();
                }
                if let Ok(mut source) = shared.source.lock() {
                    source.buffer_mut().reset();
                }
                if let Ok(mut slot) = shared.drift.lock() {
                    *slot = (None, 0.0);
                }
            }
        }

        // Resample before the packet enters the buffer, not on the way out:
        // the audio callback is realtime and a resampler whose output length
        // varies is exactly what it must not contain.
        let pcm = match resampler.as_mut() {
            Some(resampler) => match resampler.process(&pcm) {
                Ok(resampled) => resampled.to_vec(),
                // A chunk that does not match what the resampler was built for
                // is passed through uncorrected rather than dropped. Slightly
                // wrong timing beats a hole in the audio.
                Err(_) => pcm,
            },
            None => pcm,
        };

        if let Ok(mut source) = shared.source.lock() {
            if changed {
                *source = JitterSource::new(
                    JitterBuffer::new(format, JitterConfig::for_transport(link.link())),
                    format,
                );
            }
            // Read rather than discarded. The buffer keeps the packet
            // either way now, but at its packet ceiling it had to throw
            // older audio out to do so, and that is worth saying out loud:
            // nothing a healthy session does gets anywhere near it.
            if let PushOutcome::AcceptedShedding(discarded) =
                source.buffer_mut().push(sequence, timestamp, arrival, pcm)
            {
                ceiling_packets += discarded;
            }

            // Move what the buffer will release into the queue the callback
            // reads. This is the only place the jitter buffer is touched now;
            // the callback never sees it, which is the point.
            //
            // One packet out per packet in, more only to make up a queue that
            // is genuinely short, and none at all while it is more than a
            // packet deep. Draining faster than the packet rate empties the
            // jitter buffer, and an empty buffer stops playing, refills to its
            // target and releases the lot in a burst -- so the latency swung
            // between roughly nothing and the full target on a cycle, and the
            // starve at the bottom of each cycle put concealment into audio
            // that had arrived perfectly intact. That is the crackle, and it
            // is why this is a rate and not a maximum.
            //
            // Handing over nothing when the queue is already deep is the half
            // that was missing: one in and one out stops the queue growing but
            // holds it at whatever depth a startup burst left it, which on the
            // measured device was 110 ms for the whole session. It cannot
            // reintroduce the crackle, because it only ever hands over less,
            // and only above a depth the callback is already comfortable at.
            //
            // The ceiling stays because the loop cannot be written as "drain
            // until empty": a jitter buffer with a gap conceals it and hands
            // back audio, so it is always willing to produce another packet.
            // The first version of this spun forever on the first packet and
            // never went back to the socket.
            let packet_ms = packet_duration_ms(format);

            if let Ok(mut queue) = shared.queue.lock() {
                if let Some(queue) = queue.as_mut() {
                    // What the two buffers hold together is what the listener
                    // waits through, and the pacing rule below decides only
                    // which of them holds it. The bound on the sum is checked
                    // here because here is the one place both depths are known
                    // at once, and on every arrival because a bound that is
                    // sampled four times a second is a bound the depth can
                    // overshoot forty packets before anyone looks. It costs
                    // two comparisons on a thread that has just done a socket
                    // read; the audio callback is not involved and cannot be.
                    shed_packets += source.buffer_mut().shed_over_budget(queue.queued_ms());

                    let allowance = drain_allowance(queue.queued_ms(), packet_ms, PACING);

                    for _ in 0..allowance {
                        if !drain_packet(&mut source, &mut staging) {
                            break;
                        }
                        if queue.push(&staging) < staging.len() {
                            // The callback has stalled. Stop feeding rather
                            // than spinning; the resync below deals with the
                            // backlog once it is genuinely hopeless.
                            break;
                        }
                    }
                }
            }
        }

        // Reports go back to whoever sent the audio, on the address the
        // datagram came from. A sender behind NAT, or one that bound an
        // ephemeral port, is reachable this way and by no other.
        //
        // Sent before the timestamp below is updated, so the echo names a
        // packet the receiver has finished with rather than the one still
        // being handled, and the hold time it reports is real.
        if last_report.elapsed() >= Duration::from_millis(FEEDBACK_INTERVAL_MS) {
            last_report = std::time::Instant::now();
            send_report(
                socket,
                from,
                shared,
                last_accepted,
                feedback_sealer.as_mut(),
                &mut report_buffer,
            );
        }

        // Echoed back so the sender can measure a round trip against its own
        // clock. Neither end has to interpret the other's.
        last_accepted = Some((timestamp, std::time::Instant::now()));

        since_correction += 1;
        if since_correction >= PACKETS_PER_CORRECTION {
            since_correction = 0;
            let drift_ppm = estimator.as_ref().and_then(DriftEstimator::drift_ppm);

            // A buffer that keeps growing means nothing is draining it, which
            // is what the audio callback is for. Reported here because it is
            // the one symptom that distinguishes a device that never started
            // from a link that is merely fast.
            //
            // Read before the controller runs and released again immediately:
            // the drain above holds the source lock while taking this one, so
            // taking them in the other order here would be the two halves of a
            // deadlock.
            //
            // `None` on a build with no audio device, where there is no queue
            // and therefore no stage to account for. Zero would be a claim that
            // one exists and is empty.
            let queued_ms = shared.queue.lock().ok().and_then(|queue| {
                queue
                    .as_ref()
                    .map(sonduit_core::handoff::Producer::queued_ms)
            });

            // The floor the hand-off is paced to: the queue's share of the
            // audio the receiver is meant to be holding. It goes on the
            // target for the same reason the depth above goes on the depth,
            // and only when there is a queue to hold it.
            let queue_floor_ms = queued_ms.map_or(0.0, |_| {
                QUEUE_FLOOR_PACKETS as f64 * packet_duration_ms(format)
            });

            // The sum, not the jitter buffer alone. Audio waiting in the queue
            // is latency the listener hears exactly as much as audio waiting
            // in the buffer, and the controller is the only thing that can
            // shed either without a click. Given the depth of one half it held
            // the other half wherever it happened to be: a session was
            // measured carrying 110 ms in the queue for four minutes with the
            // correction doing nothing about it, because from here it did not
            // exist.
            if let Ok(source) = shared.source.lock() {
                controller.update(
                    source.buffer().depth_ms() + queued_ms.unwrap_or(0.0),
                    source.buffer().target_ms() + queue_floor_ms,
                    drift_ppm,
                );
            }
            if let Some(resampler) = resampler.as_mut() {
                resampler.set_ratio(controller.ratio());
            }
            if let Ok(mut slot) = shared.drift.lock() {
                *slot = (drift_ppm, controller.correction_ppm());
            }

            let target_ms = shared
                .source
                .lock()
                .map_or(30.0, |source| source.buffer().target_ms());

            // Resampling shifts parts per million. It cannot shed a backlog,
            // and on the first real device it sat pinned at its 500 ppm limit
            // against 1536 ms of queued audio, which would have taken fifty
            // minutes to clear.
            //
            // Measured against the jitter buffer's target rather than the
            // queue's own floor, which is deliberately the more conservative
            // of the two: the pacing above keeps the queue near its floor
            // without dropping anything, so by the time the queue is four
            // times the *jitter* target deep the callback has stopped running
            // altogether, which is what this path is for.
            let mut dropped = 0;
            if let Ok(mut queue) = shared.queue.lock() {
                if let Some(queue) = queue.as_mut() {
                    dropped = queue.resync_if_hopeless(target_ms);
                }
            }
            if dropped > 0 {
                note!(
                    "resynchronised: dropped {dropped} frames from {:.0} ms queued",
                    queued_ms.unwrap_or(0.0)
                );
            }

            // The same emergency, reported from the other buffer. The pacing
            // rule holds the queue within a packet of its floor, so the queue
            // can no longer reach four times the jitter target and the
            // backstop above can no longer see this failure at all: the
            // surplus collects in the jitter buffer now, and the bound on the
            // drain path is what catches it there.
            if shed_packets > 0 {
                note!(
                    "shed {shed_packets} packets: the receiver was holding past what this link allows"
                );
                shed_packets = 0;
            }

            // And the floor underneath that one. The bound above is checked on
            // every arrival, so this can only be reached when it was not: the
            // buffer filled to its packet ceiling with nothing measuring it.
            // A session that logs this is holding audio nobody bounded, and
            // the line is here to say which of the two limits gave way.
            if ceiling_packets > 0 {
                note!(
                    "shed {ceiling_packets} packets at the jitter buffer's packet ceiling: the bound on the total never ran"
                );
                ceiling_packets = 0;
            }

            // Worth a line, and worth being specific: on a healthy paired link
            // this is zero, and anything else is a sender using the wrong key,
            // a sender that has not been paired with, or somebody injecting
            // datagrams. None of the three is a fault in this device.
            if refused_since_report > 0 {
                note!("refused {refused_since_report} datagrams the session key does not accept");
                refused_since_report = 0;
            }

            // A stream that has gone cannot be restarted, and AAudio stops
            // calling back without saying anything. Reopening is the only
            // recovery, and it needs a fresh queue with it.
            if playback_disconnected(shared) {
                note!("playback stream disconnected; reopening");
                open_playback(shared, format);
            }

            // Both depths and both targets. The log used to print the queue's
            // depth beside the jitter buffer's target, which reads as a queue
            // running three times deep when the two numbers are about
            // different buffers entirely.
            note!(
                "depth {:.0} ms + queued {:.0} ms, target {target_ms:.0} ms + {queue_floor_ms:.0} ms, drift {:?} ppm, correction {:.0} ppm, frames played {}",
                shared
                    .source
                    .lock()
                    .map_or(0.0, |source| source.buffer().depth_ms()),
                queued_ms.unwrap_or(0.0),
                drift_ppm.map(|ppm| ppm.round()),
                controller.correction_ppm(),
                playback_frames(shared)
            );
        }

        if changed {
            // The device is opened here, not in start, because only now is the
            // rate known. Reopening on a later change is the same code path: a
            // sender that switches format is a new stream, and an AAudio
            // stream cannot change rate once opened.
            open_playback(shared, format);
        }

        if !seen_audio {
            seen_audio = true;
            // Not if the device refused to open. Streaming set unconditionally
            // here overwrote the Failed state open_playback had just set one
            // line earlier, so a receiver that could not play anything
            // reported itself as playing.
            if read_state(shared) != BridgeState::Failed {
                set_state(shared, BridgeState::Streaming);
            }
            note!(
                "first packet accepted: {} Hz, {} ch",
                format.sample_rate,
                format.channels
            );
        }
    }

    set_state(shared, BridgeState::Idle);
}

/// Open, or reopen, the audio device for a format the sender has confirmed.
///
/// A failure is recorded rather than propagated: the receive thread has no
/// caller to return to, and a session that keeps buffering while the device is
/// unavailable still reports honestly through telemetry. The alternative,
/// tearing the session down, loses the diagnosis with it.
#[cfg(target_os = "android")]
fn open_playback(shared: &Arc<Shared>, format: Format) {
    // Room for well over the jitter buffer's own target, so an ordinary burst
    // never reaches the ceiling, but bounded so a stalled callback cannot let
    // the latency grow without limit.
    let (producer, consumer) = sonduit_core::handoff::channel(format, 400);

    let Ok(mut slot) = shared.playback.lock() else {
        return;
    };
    if let Some(previous) = slot.take() {
        let _ = previous.stop();
    }
    if let Ok(mut queue) = shared.queue.lock() {
        *queue = Some(producer);
    }
    let source = consumer;

    match sonduit_playback_android::Playback::open(format, source) {
        Ok(playback) => {
            // What the device granted, which is not what was asked for and is
            // the single largest open question in docs/latency-budget.md.
            // AAudio does not fail a request it cannot honour; it succeeds
            // with something worse and says nothing.
            let granted = playback.granted();
            note!(
                "playback open: {} Hz, {} ch, burst {} frames, exclusive {}, low latency {}, buffer {:.1} ms",
                granted.format.sample_rate,
                granted.format.channels,
                granted.frames_per_burst,
                granted.exclusive,
                granted.low_latency,
                playback.buffer_latency_ms()
            );
            *slot = Some(playback);
            if let Ok(mut error) = shared.playback_error.lock() {
                *error = None;
            }
        }
        Err(error) => {
            note!("playback open failed: {error}");
            if let Ok(mut slot) = shared.playback_error.lock() {
                *slot = Some(error.to_string());
            }
            set_state(shared, BridgeState::Failed);
        }
    }
}

/// Off Android there is no device to open, and the receive path is still worth
/// running: it is how the transport is tested on a development machine.
#[cfg(not(target_os = "android"))]
fn open_playback(_shared: &Arc<Shared>, _format: Format) {}

/// Answer discovery probes so the desktop can find this device.
///
/// Each reply is tagged against the probe's own nonce, so it proves this
/// device knows the pairing code without putting the code on the wire.
///
/// Runs for as long as the app does. It is not part of playing audio and it
/// does not touch the audio path: no session state is reachable from here, and
/// nothing here is on a realtime thread.
fn announce_loop(
    socket: &UdpSocket,
    stop: &AtomicBool,
    name: &Mutex<String>,
    audio_port: &AtomicU16,
    code: &Mutex<PairingCode>,
    keys: &Keys,
) {
    let mut datagram = [0_u8; 256];
    // Nonces of the probes recently answered, newest last. The key offer that
    // follows a scan is tagged against the nonce of the probe it belongs to,
    // and this device cannot tell from the offer alone which probe that was.
    // A nonce is not a secret: every probe carried its own in the clear.
    let mut recent: Vec<[u8; NONCE_BYTES]> = Vec::with_capacity(REMEMBERED_NONCES);
    // What has already been answered, so the copies of one offer are answered
    // identically. Without it the desktop keeps the key from the first copy
    // and this device keeps the key from the last, and every packet of the
    // session is refused. See `handshake::Responder`.
    let mut responder = handshake::Responder::new();

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };

        // The second half of the typed-code pairing path: the desktop follows
        // a verified announcement with its half of the key agreement, and this
        // answers it. Without this a scan would find a device it then could
        // not send audio to, because the desktop refuses to stream to a peer
        // it holds no key for.
        if handshake::is_key_offer(&datagram[..length]) {
            answer_offer(
                socket,
                from,
                &datagram[..length],
                code,
                keys,
                &recent,
                &mut responder,
            );
            continue;
        }

        // A probe with no readable nonce is either malformed or an older
        // protocol version, and there is nothing to authenticate against
        // either way.
        let Some(nonce) = discovery::probe_nonce(&datagram[..length]) else {
            continue;
        };

        // Remembered before the reply goes out, so an offer that arrives
        // immediately after it has something to verify against.
        if !recent.contains(&nonce) {
            if recent.len() == REMEMBERED_NONCES {
                recent.remove(0);
            }
            recent.push(nonce);
        }

        // All three are read per probe rather than captured once. Scanning a
        // desktop's QR replaces the code, starting a session replaces the
        // port, and the app learns its own name after this thread is already
        // running: a reply built from what was true at startup would name the
        // wrong port under a key the desktop cannot verify.
        let name = {
            let Ok(name) = name.lock() else {
                return;
            };
            name.clone()
        };
        let port = audio_port.load(Ordering::Relaxed);
        let Ok(code) = code.lock() else {
            return;
        };

        // The reply is built per probe rather than once, because the tag
        // covers that probe's nonce. That is what stops it being replayed.
        let reply = discovery::encode_announce(&name, port, &nonce, &code);
        drop(code);

        // Straight back to the prober rather than broadcast: the answer
        // concerns one machine, and broadcasting it would wake every device on
        // the network and hand them all a tag to study.
        let _ = socket.send_to(&reply, from);
    }
}

/// Answer a key offer against whichever recent probe it belongs to.
///
/// Tried against every nonce this responder has answered lately, because the
/// offer names none of them and the tag is what decides. An offer that
/// verifies against none of them is dropped in silence, exactly as an
/// announcement that does not verify is: it is either a device that does not
/// hold this code or a stray datagram, and saying which would tell an attacker
/// which of its guesses was closer.
///
/// The desktop sends its offer three times, so `responder` carries what was
/// answered before: the second and third copies get the accept the first one
/// got, and the key this device adopted stays the key the desktop derived. It
/// is the caller's `responder` rather than one made here because a responder
/// that forgot between datagrams would remember nothing at all.
///
/// A failure to read the system's random source is logged and the offer is
/// left unanswered. The desktop then reports that the device did not pair,
/// which is true, rather than this device pairing under a key pair generated
/// from something guessable.
fn answer_offer(
    socket: &UdpSocket,
    from: SocketAddr,
    offer: &[u8],
    code: &Mutex<PairingCode>,
    keys: &Keys,
    recent: &[[u8; NONCE_BYTES]],
    responder: &mut handshake::Responder,
) {
    // Cloned and the lock released at once: the announce thread must not hold
    // the code while it does a key agreement, and the UI reads the same mutex
    // to put the digits on screen.
    let code = {
        let Ok(guard) = code.lock() else {
            return;
        };
        guard.clone()
    };

    let seed = match entropy::key_seed() {
        Ok(seed) => seed,
        Err(error) => {
            note!("no random seed for the key agreement, pairing refused: {error}");
            return;
        }
    };

    let Some(answered) = responder.answer(offer, recent, &code, seed) else {
        return;
    };
    let _ = socket.send_to(&answered.accept, from);

    // Absent on a repeat, and a repeat must not re-adopt: the desktop that
    // sent this offer either holds the key already or has been replaced by
    // one that pairs later, and re-adopting would take that newer pairing
    // down on a datagram anybody could have kept and sent again.
    if let Some(secret) = answered.secret {
        keys.adopt(secret);
        note!("paired by scan; audio from this computer will be encrypted");
    }
}

/// Milliseconds as the tenths the feedback report is encoded in.
///
/// Clamped rather than wrapped, for the same reason the hold time is: a figure
/// past the top of the range means something is wrong, and folding it back to
/// a small number would present that as healthy.
fn tenths_of_a_millisecond(ms: f64) -> u16 {
    (ms * 10.0).clamp(0.0, f64::from(u16::MAX)) as u16
}

/// One packet's duration in milliseconds at `format`.
///
/// Zero for a format whose payload does not divide into whole frames, which is
/// a format nothing can play. Every caller treats zero as "not known yet"
/// rather than dividing by it.
fn packet_duration_ms(format: Format) -> f64 {
    format
        .packet_duration_nanos()
        .map_or(0.0, |nanos| nanos as f64 / 1_000_000.0)
}

/// Send one report to the sender.
///
/// Failures are ignored on purpose. A report that does not arrive costs the
/// sender one missed sample of a figure it redraws four times a second, and
/// the alternative, tearing down a session that is playing audio correctly
/// because a status datagram was refused, is plainly worse.
/// `sealer` is present exactly when this device holds a pairing key. A keyed
/// session's reports are sealed under a key derived with its own label, so a
/// report can never be replayed into the audio path or the reverse, and the
/// sender refuses a cleartext one. An unkeyed session sends the version 1
/// encoding, which is the only thing an unkeyed sender can read.
fn send_report(
    socket: &UdpSocket,
    to: SocketAddr,
    shared: &Arc<Shared>,
    last_accepted: Option<(u32, std::time::Instant)>,
    sealer: Option<&mut FeedbackSealer>,
    buffer: &mut [u8; SEALED_FEEDBACK_BYTES],
) {
    let Some((echo, accepted_at)) = last_accepted else {
        return;
    };

    // The hand-off queue as well as the jitter buffer. Audio crosses both, and
    // a report that carried only the first described a fraction of the delay
    // it claimed to describe: a measured session held 110 ms here, behind a
    // 42 ms buffer, and the sender was told about the 42.
    //
    // try_lock, not lock: this runs on the receive thread and must not wait
    // behind the audio callback for a status message. Taken and released
    // before the source lock, never nested inside it, because the drain path
    // holds the source and reaches for this one.
    let queued_ms = shared.queue.try_lock().ok().and_then(|queue| {
        queue
            .as_ref()
            .map(sonduit_core::handoff::Producer::queued_ms)
    });

    let Ok(source) = shared.source.try_lock() else {
        return;
    };
    let stats = source.buffer().stats();
    let depth_ms = source.buffer().depth_ms();
    drop(source);

    let report = Feedback {
        echo,
        // Clamped rather than wrapped. A hold longer than a minute means
        // something has gone badly wrong, and reporting it as a small number
        // would hide that.
        hold_ms: accepted_at.elapsed().as_millis().min(u128::from(u16::MAX)) as u16,
        accepted: stats.accepted,
        lost: stats.lost,
        depth_tenths_ms: tenths_of_a_millisecond(depth_ms),
        queue_tenths_ms: queued_ms.map(tenths_of_a_millisecond),
        playing: read_state(shared) == BridgeState::Streaming,
    };

    match sealer {
        Some(sealer) => {
            if let Ok(length) = sealer.seal(&report, buffer) {
                let _ = socket.send_to(&buffer[..length], to);
            }
        }
        None => {
            if let Ok(length) = report.encode(buffer) {
                let _ = socket.send_to(&buffer[..length], to);
            }
        }
    }
}

/// Frames the audio device has pulled so far.
///
/// Zero while packets keep arriving is the signature of a stream that opened
/// and never ran, which is otherwise indistinguishable from a fast link.
#[cfg(target_os = "android")]
fn playback_frames(shared: &Arc<Shared>) -> u64 {
    shared
        .playback
        .try_lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|p| p.counters().frames_played()))
        .unwrap_or(0)
}

/// Off Android nothing plays, so nothing has been pulled.
#[cfg(not(target_os = "android"))]
fn playback_frames(_shared: &Arc<Shared>) -> u64 {
    0
}

/// Whether AAudio has reported the output stream gone.
#[cfg(target_os = "android")]
fn playback_disconnected(shared: &Arc<Shared>) -> bool {
    shared.playback.try_lock().ok().is_some_and(|slot| {
        slot.as_ref()
            .is_some_and(sonduit_playback_android::Playback::disconnected)
    })
}

/// Off Android there is no stream to lose.
#[cfg(not(target_os = "android"))]
fn playback_disconnected(_shared: &Arc<Shared>) -> bool {
    false
}

/// Decide which link the audio is arriving over.
///
/// The sender declares it, in flag bit 0 of the packet header
/// ([`sonduit_core::packet::FLAG_WIRED_LINK`]). Only the sender can answer
/// this, because it is the end that picked the interface. A set flag means a
/// wired link and is taken at face value.
///
/// A clear flag is not a claim of Wi-Fi, only a sender declining to say, which
/// is also what every sender predating the flag does. That is the one case
/// that falls back to guessing from the source address, and the guess is right
/// only by luck: USB tethering has no reserved range, and a real phone here
/// handed out 10.114.89.x rather than the 192.168.42/24 stock Android uses.
///
/// Either way, the cost of being wrong is a buffer sized for the other link,
/// which is 20 ms of latency or a few dropouts, not a broken session.
fn link_of(from: SocketAddr, declared_wired: bool) -> Transport {
    if declared_wired {
        return Transport::Usb;
    }

    // Nothing was declared, so fall back to the address. This is only ever
    // right by luck -- USB tethering has no reserved range, and a phone here
    // handed out 10.114.89.x -- but a sender too old to set the flag is still
    // worth serving, and 192.168.42/24 is what stock Android uses.
    match from.ip() {
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            if octets[0] == 192 && octets[1] == 168 && octets[2] == 42 {
                Transport::Usb
            } else {
                Transport::WiFi
            }
        }
        std::net::IpAddr::V6(_) => Transport::WiFi,
    }
}

/// A seed for a pairing code.
///
/// Not a cryptographic generator, and it does not need to be one: the code is
/// six digits shown on a screen and typed by hand, so its strength is bounded
/// by that regardless. What it does need is to be unpredictable to someone who
/// is not looking at the screen. The clock supplies the entropy, the stack
/// address supplies whatever ASLR gives, and the counter stops two codes
/// generated in the same nanosecond from matching.
fn random_seed() -> u64 {
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as u64);
    let local = 0_u8;
    let address = std::ptr::addr_of!(local) as u64;
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

    nanos
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(address.rotate_left(17))
        .wrapping_add(counter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probe a responder and return what it announced, if anything verified.
    ///
    /// Retried rather than sent once: this is UDP on the loopback interface,
    /// which does not lose datagrams often but is not obliged to keep any of
    /// them. Twenty attempts at a quarter of a second each is five seconds
    /// before it gives up, which is long enough that a failure here is the
    /// responder and not the timing.
    #[cfg(not(target_os = "android"))]
    fn probe(discovery_port: u16, code: &PairingCode) -> Option<discovery::Announcement> {
        let nonce = [0xA7; sonduit_transport::pairing::NONCE_BYTES];
        let prober = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).ok()?;
        prober
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok()?;
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery_port));
        let mut datagram = [0_u8; 256];

        for _ in 0..20 {
            if prober
                .send_to(&discovery::encode_probe(&nonce), target)
                .is_err()
            {
                continue;
            }
            let Ok((length, _)) = prober.recv_from(&mut datagram) else {
                continue;
            };
            if let Some(found) = discovery::decode_announce(&datagram[..length], &nonce, code) {
                return Some(found);
            }
        }
        None
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn an_idle_bridge_answers_a_probe_carrying_its_own_code() {
        // The bug this exists for. The responder used to be spawned by start,
        // so a phone that was open, showing its six digits and not yet started
        // answered nothing at all, and the desktop's typed-code scan reported
        // that there was no device on the network. The QR flow hid it, because
        // there the phone sends and never has to be listening.
        let discovery_port = 41_028;
        let bridge = Bridge::with_discovery_port(discovery_port);
        bridge.set_device_name("Pixel 8".to_string());

        // Nothing is started. That is the whole point of the test.
        assert_eq!(bridge.state(), BridgeState::Idle);

        let code = PairingCode::parse(&bridge.pairing_code()).expect("the code is six digits");
        let announcement =
            probe(discovery_port, &code).expect("an idle bridge must answer a probe for its code");

        assert_eq!(announcement.name, "Pixel 8");
        assert_eq!(
            announcement.audio_port, DEFAULT_PORT,
            "an idle bridge names the port the next session will bind"
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_probe_for_the_wrong_code_is_not_answered_by_an_idle_bridge() {
        // Being findable while idle must not mean being findable by anyone.
        let discovery_port = 41_030;
        let bridge = Bridge::with_discovery_port(discovery_port);

        let mine = PairingCode::parse(&bridge.pairing_code()).expect("the code is six digits");
        let theirs = if mine == PairingCode::parse("482913").unwrap() {
            PairingCode::parse("000001").unwrap()
        } else {
            PairingCode::parse("482913").unwrap()
        };

        assert!(probe(discovery_port, &theirs).is_none());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_session_starting_and_stopping_leaves_the_responder_answering() {
        // One socket, one owner: start must not bind a second responder, and
        // stop must not take the only one away. What changes across a session
        // is the port the announcement names, and nothing else.
        let discovery_port = 41_029;
        let audio_port = 41_012;
        let bridge = Bridge::with_discovery_port(discovery_port);
        let code = PairingCode::parse(&bridge.pairing_code()).expect("the code is six digits");

        let idle = probe(discovery_port, &code).expect("answers before a session");
        assert_eq!(idle.audio_port, DEFAULT_PORT);

        if bridge.start(audio_port).is_err() {
            return;
        }
        let running = probe(discovery_port, &code).expect("still answers during a session");
        assert_eq!(
            running.audio_port, audio_port,
            "a running session names the port it actually bound"
        );

        bridge.stop().unwrap();
        let after = probe(discovery_port, &code).expect("still answers after a session");
        assert_eq!(after.audio_port, DEFAULT_PORT);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn dropping_a_bridge_frees_the_discovery_port_for_the_next_one() {
        // A detached responder would hold the port and the next Bridge in the
        // process would be silently unfindable, which is the failure this
        // change is about wearing a different hat.
        let discovery_port = 41_031;

        let first = Bridge::with_discovery_port(discovery_port);
        let code = PairingCode::parse(&first.pairing_code()).expect("the code is six digits");
        assert!(probe(discovery_port, &code).is_some());
        drop(first);

        let second = Bridge::with_discovery_port(discovery_port);
        let code = PairingCode::parse(&second.pairing_code()).expect("the code is six digits");
        assert!(
            probe(discovery_port, &code).is_some(),
            "the port was still held by the dropped bridge"
        );
    }

    /// Pair with a bridge over loopback exactly as the desktop's typed-code
    /// scan does: probe, verify the announcement, offer a key, take the
    /// accept.
    ///
    /// The real four datagrams over real sockets. A test that reached inside
    /// and installed a secret would prove the cipher works and nothing about
    /// the path a pairing actually takes, which is the half of this that the
    /// two ends have to agree on.
    #[cfg(not(target_os = "android"))]
    fn pair_over_loopback(discovery_port: u16, code: &PairingCode) -> SessionSecret {
        use sonduit_transport::handshake::Offer;
        use sonduit_transport::session::SEED_BYTES;

        let nonce = [0xA7_u8; NONCE_BYTES];
        let desktop = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("an ephemeral loopback socket");
        desktop
            .set_read_timeout(Some(Duration::from_millis(250)))
            .expect("a fresh socket takes a timeout");
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery_port));
        let mut datagram = [0_u8; 256];

        // Step one: find it, which is the exchange that already existed.
        let mut responder = None;
        for _ in 0..20 {
            if desktop
                .send_to(&discovery::encode_probe(&nonce), target)
                .is_err()
            {
                continue;
            }
            let Ok((length, from)) = desktop.recv_from(&mut datagram) else {
                continue;
            };
            if discovery::decode_announce(&datagram[..length], &nonce, code).is_some() {
                responder = Some(from);
                break;
            }
        }
        let responder = responder.expect("the bridge must answer a probe for its own code");

        // Step two: agree a key with it, which is what this change adds.
        //
        // Three offers before the listen, because that is what the desktop
        // sends and one offer would not exercise the thing that broke: a
        // responder answering each copy with its own key pair leaves this end
        // holding the key from the copy that arrived first and the bridge
        // holding the key from the copy that arrived last, and every packet of
        // the session that follows is refused. So every accept that comes back
        // is collected and they are required to be the same bytes.
        let offer = Offer::new([0x5D; SEED_BYTES], nonce, code.clone());
        let mut accepts: Vec<Vec<u8>> = Vec::new();
        for _ in 0..3 {
            let _ = desktop.send_to(&offer.datagram(), responder);
        }
        for _ in 0..20 {
            let Ok((length, _)) = desktop.recv_from(&mut datagram) else {
                if accepts.is_empty() {
                    let _ = desktop.send_to(&offer.datagram(), responder);
                }
                continue;
            };
            if offer.is_our_accept(&datagram[..length]) {
                accepts.push(datagram[..length].to_vec());
                if accepts.len() == 3 {
                    break;
                }
            }
        }
        assert!(!accepts.is_empty(), "the bridge must answer a key offer");
        for accept in &accepts {
            assert_eq!(
                accept, &accepts[0],
                "the bridge answered one offer with two different keys"
            );
        }

        offer
            .accept(&accepts[0])
            .expect("the accept must complete the agreement")
    }

    /// A cleartext Sonduit datagram, as an old sender or an attacker sends it.
    #[cfg(not(target_os = "android"))]
    fn cleartext(sequence: u16, pcm: &[u8]) -> Vec<u8> {
        let mut datagram = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        SonduitPacket {
            format: Format::stereo_48k(),
            sequence,
            timestamp_frames: u32::from(sequence) * (pcm.len() / 4) as u32,
            flags: 0,
            pcm,
        }
        .encode(&mut datagram)
        .expect("a packet of the right size encodes");
        datagram
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_paired_receiver_plays_sealed_audio_and_refuses_the_same_audio_in_the_clear() {
        // Both halves of the downgrade defence in one session, because they
        // are one property: audio is played only when the key says so, and the
        // key saying no is never a reason to play it anyway.
        use sonduit_core::format::PCM_PAYLOAD_BYTES;
        use sonduit_transport::sealed::Sealer;
        use sonduit_transport::session::SALT_BYTES;

        let bridge = Bridge::with_discovery_port(41_040);
        let port = 41_041;
        if bridge.start(port).is_err() {
            return;
        }

        let code = PairingCode::parse(&bridge.pairing_code()).expect("the code is six digits");
        let secret = pair_over_loopback(41_040, &code);
        assert!(
            bridge.is_paired(),
            "the bridge did not keep the key it agreed"
        );

        let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let pcm = vec![7_u8; PCM_PAYLOAD_BYTES];

        let mut sealer = Sealer::new(&secret, [0x4E; SALT_BYTES]);
        let mut sealed = vec![0_u8; Sealer::sealed_len(pcm.len())];
        for _ in 0..8 {
            sealer
                .seal(&Format::stereo_48k(), 0, 0, &pcm, &mut sealed)
                .expect("sealing cannot fail for a well sized buffer");
            // The bytes on the wire are not the audio. Asserted here as well
            // as in the transport tests, because this is the datagram the
            // application actually sends.
            assert!(
                !sealed.windows(pcm.len()).any(|window| window == pcm),
                "the PCM went out in the clear"
            );
            sender.send_to(&sealed, target).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));

        let playing = bridge.telemetry();
        assert_eq!(playing.packets_accepted, 8, "sealed audio was not played");
        assert_eq!(playing.packets_refused, 0);
        assert!(
            playing.encrypted,
            "a keyed session must report itself keyed"
        );

        // The same audio, in the clear, from the same address. A receiver that
        // accepted this would make the encryption a suggestion: an attacker
        // would simply send version 1.
        for sequence in 0..4_u16 {
            sender.send_to(&cleartext(sequence, &pcm), target).unwrap();
        }
        // And the other wire, which has no version field and no key at all.
        let mut scream = vec![0_u8; sonduit_core::packet::SCREAM_PACKET_BYTES];
        ScreamPacket::encode(&Format::stereo_48k(), &pcm, &mut scream).unwrap();
        sender.send_to(&scream, target).unwrap();
        std::thread::sleep(Duration::from_millis(300));

        let after = bridge.telemetry();
        bridge.stop().unwrap();

        assert_eq!(
            after.packets_accepted, 8,
            "cleartext audio reached a keyed receiver"
        );
        assert_eq!(
            after.packets_refused, 5,
            "the refusals were not counted, so nobody would ever see them"
        );
        assert_eq!(
            after.packets_malformed, 0,
            "a refused packet is not a malformed one and must not be filed as one"
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn an_unpaired_receiver_refuses_sealed_audio_rather_than_playing_it() {
        // The other direction, and the one that would be a noise burst: a
        // receiver with no key that treated ciphertext as PCM would play it at
        // full scale into somebody's headphones.
        use sonduit_core::format::PCM_PAYLOAD_BYTES;
        use sonduit_transport::handshake::{Offer, Responder};
        use sonduit_transport::sealed::Sealer;
        use sonduit_transport::session::{SALT_BYTES, SEED_BYTES};

        let bridge = Bridge::with_discovery_port(41_042);
        let port = 41_043;
        if bridge.start(port).is_err() {
            return;
        }
        assert!(!bridge.is_paired());

        // A key agreed between two ends that are not this device.
        let nonce = [0x11_u8; NONCE_BYTES];
        let code = PairingCode::parse("482913").unwrap();
        let offer = Offer::new([1; SEED_BYTES], nonce, code.clone());
        let accept = Responder::new()
            .answer(&offer.datagram(), &[nonce], &code, [2; SEED_BYTES])
            .expect("well formed")
            .accept;
        let stranger = offer.accept(&accept).expect("a complete agreement");

        let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let pcm = vec![7_u8; PCM_PAYLOAD_BYTES];
        let mut sealer = Sealer::new(&stranger, [0x77; SALT_BYTES]);
        let mut sealed = vec![0_u8; Sealer::sealed_len(pcm.len())];
        for _ in 0..6 {
            sealer
                .seal(&Format::stereo_48k(), 0, 0, &pcm, &mut sealed)
                .unwrap();
            sender.send_to(&sealed, target).unwrap();
        }

        std::thread::sleep(Duration::from_millis(300));
        let telemetry = bridge.telemetry();
        bridge.stop().unwrap();

        assert_eq!(
            telemetry.packets_accepted, 0,
            "ciphertext was decoded as audio"
        );
        assert_eq!(
            telemetry.packets_refused, 6,
            "the refusals were not counted"
        );
        assert!(!telemetry.encrypted);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_new_pairing_code_throws_the_key_away_with_it() {
        // The one way back to an unkeyed receiver, and the only reason a user
        // who wants an unmodified Scream sender is not stuck. It has to
        // actually clear the key, or the device would keep refusing.
        let bridge = Bridge::with_discovery_port(41_044);
        let code = PairingCode::parse(&bridge.pairing_code()).expect("the code is six digits");
        let _ = pair_over_loopback(41_044, &code);
        assert!(bridge.is_paired());

        bridge.regenerate_pairing_code();
        assert!(
            !bridge.is_paired(),
            "the key outlived the code it was agreed under"
        );
    }

    #[test]
    fn a_fresh_bridge_is_idle_and_reports_empty_telemetry() {
        let bridge = Bridge::with_discovery_port(41_020);
        assert_eq!(bridge.state(), BridgeState::Idle);

        let telemetry = bridge.telemetry();
        assert_eq!(telemetry.packets_accepted, 0);
        assert_eq!(telemetry.sample_rate, 0);
    }

    #[test]
    fn stopping_a_stopped_bridge_is_not_an_error() {
        // The Android service lifecycle calls stop from more than one place.
        let bridge = Bridge::with_discovery_port(41_021);
        assert!(bridge.stop().is_ok());
        assert!(bridge.stop().is_ok());
    }

    #[test]
    fn the_announced_name_can_be_set_before_starting() {
        let bridge = Bridge::with_discovery_port(41_022);
        bridge.set_device_name("Pixel".to_string());
        assert_eq!(*bridge.device_name.lock().unwrap(), "Pixel");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_session_receives_and_buffers_real_datagrams() {
        // No audio device is opened off Android, so this exercises the socket,
        // the classifier, the format latch and the jitter buffer, which is
        // every part of the receive path that is not the device itself.
        use sonduit_core::format::PCM_PAYLOAD_BYTES;

        let bridge = Bridge::with_discovery_port(41_023);
        // Port zero would bind an ephemeral port the sender cannot guess, so a
        // fixed high port is used; a bind failure here means something else
        // holds it, and the test reports that rather than hanging.
        let port = 41_010;
        if bridge.start(port).is_err() {
            return;
        }

        let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let pcm = vec![7_u8; PCM_PAYLOAD_BYTES];
        let mut datagram = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];

        for sequence in 0..8_u16 {
            SonduitPacket {
                format: Format::stereo_48k(),
                sequence,
                timestamp_frames: u32::from(sequence) * (PCM_PAYLOAD_BYTES / 4) as u32,
                flags: 0,
                pcm: &pcm,
            }
            .encode(&mut datagram)
            .unwrap();
            sender.send_to(&datagram, target).unwrap();
        }

        // The receive thread is blocked on recv_from; give it a moment to
        // drain what was just sent.
        std::thread::sleep(Duration::from_millis(300));

        let telemetry = bridge.telemetry();
        bridge.stop().unwrap();

        assert_eq!(telemetry.packets_accepted, 8, "every datagram was buffered");
        assert_eq!(telemetry.packets_malformed, 0);
        assert_eq!(
            telemetry.sample_rate, 48_000,
            "format latched from the wire"
        );
        assert_eq!(telemetry.state, BridgeState::Streaming);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn junk_is_counted_as_malformed_rather_than_buffered() {
        let bridge = Bridge::with_discovery_port(41_024);
        let port = 41_011;
        if bridge.start(port).is_err() {
            return;
        }

        let sender = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        sender.send_to(&[0_u8; 40], target).unwrap();
        sender.send_to(b"not a packet", target).unwrap();

        std::thread::sleep(Duration::from_millis(300));
        let telemetry = bridge.telemetry();
        bridge.stop().unwrap();

        assert_eq!(telemetry.packets_malformed, 2);
        assert_eq!(telemetry.packets_accepted, 0);
    }

    #[test]
    fn scanned_text_that_is_not_an_invite_is_refused_rather_than_acted_on() {
        // The camera sees whatever is in frame. None of it may start pairing.
        let bridge = Bridge::with_discovery_port(41_025);
        for text in ["", "https://example.com", "SDQ9:482913:4011:A:10.0.0.2"] {
            assert!(
                matches!(
                    bridge.accept_invite(text.to_string()),
                    Err(FfiError::BadInvite)
                ),
                "accepted {text:?}"
            );
        }
    }

    #[test]
    fn pairing_before_a_session_is_listening_is_refused() {
        // The announcement advertises the port audio will arrive on, and an
        // idle bridge has not bound one. Announcing a port nothing is
        // listening on would have the desktop send audio into a closed socket
        // and report a healthy session.
        let bridge = Bridge::with_discovery_port(41_026);
        let invite = Invite::new(
            &[Ipv4Addr::new(10, 10, 0, 61)],
            discovery::DISCOVERY_PORT,
            PairingCode::parse("482913").unwrap(),
            [0x5A; sonduit_transport::pairing::NONCE_BYTES],
        )
        .unwrap();

        assert!(matches!(
            bridge.accept_invite(invite.to_payload()),
            Err(FfiError::NotRunning)
        ));
    }

    #[test]
    fn scanning_a_code_makes_this_device_answer_to_the_desktops_code() {
        // The desktop chose the code, so this device has to adopt it: a later
        // broadcast probe from the same desktop is answered with the same key
        // or the rescan finds nothing.
        let bridge = Bridge::with_discovery_port(41_027);
        let port = 41_013;
        if bridge.start(port).is_err() {
            return;
        }

        let before = bridge.pairing_code();
        let invite = Invite::new(
            &[Ipv4Addr::new(10, 10, 0, 61)],
            discovery::DISCOVERY_PORT,
            PairingCode::parse("482913").unwrap(),
            [0x5A; sonduit_transport::pairing::NONCE_BYTES],
        )
        .unwrap();

        // Either outcome is legitimate here: whether the datagram can leave
        // depends on this machine having a route, and the test is about the
        // code, not about the network.
        let _ = bridge.accept_invite(invite.to_payload());
        let after = bridge.pairing_code();
        bridge.stop().unwrap();

        assert_eq!(after, "482913");
        assert_ne!(after, before, "the seeded code should not already match");
    }
}
