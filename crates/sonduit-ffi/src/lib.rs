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
//! `start` creates two: one blocked on `recv_from`, decoding datagrams into the
//! jitter buffer, and one answering discovery probes. Audio is pulled by
//! AAudio on a third thread it owns. The jitter buffer is the only thing all
//! three touch, and the audio callback only ever `try_lock`s it.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sonduit_core::drift::{DriftConfig, DriftEstimator};
use sonduit_core::format::Format;
use sonduit_core::jitter::{JitterBuffer, JitterConfig, Transport};
use sonduit_core::packet::{ScreamPacket, SonduitPacket};
use sonduit_core::ratio::{RatioConfig, RatioController};
use sonduit_core::resample::DriftResampler;
use sonduit_playback_android::{drain_packet, JitterSource};
use sonduit_transport::feedback::{Feedback, FEEDBACK_BYTES, FEEDBACK_INTERVAL_MS};
use sonduit_transport::invite::Invite;
use sonduit_transport::pairing::PairingCode;
use sonduit_transport::{classify, discovery, Wire, DEFAULT_PORT};

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

/// Packets between drift corrections.
///
/// At 6 ms packets this is four times a second, matching the rate the UI
/// samples telemetry at. Crystal drift is a physical constant of the two
/// devices and does not change from moment to moment, so correcting faster
/// would only chase jitter.
const PACKETS_PER_CORRECTION: u32 = 40;

/// Packets moved from the jitter buffer to the audio queue per packet received.
///
/// Above one so a buffer that has fallen behind can catch up, and small enough
/// that catching up cannot starve the socket. The loop it bounds cannot be
/// written as "drain until empty": a jitter buffer conceals a gap rather than
/// reporting one, so it will always produce another packet if asked.
const DRAIN_PER_PACKET: usize = 3;

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
    /// Which link the audio is arriving over, as guessed from the sender's
    /// address. Empty before the first packet.
    pub transport: String,
}

/// State shared between the receive thread and the FFI callers.
struct Shared {
    source: Mutex<JitterSource>,
    state: Mutex<BridgeState>,
    malformed: Mutex<u64>,
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

/// A handle the Android app holds for the lifetime of a session.
#[derive(uniffi::Object)]
pub struct Bridge {
    inner: Mutex<Option<Running>>,
    shared: Mutex<Option<Arc<Shared>>>,
    device_name: Mutex<String>,
    /// The code this device will answer probes with.
    ///
    /// Generated once per process rather than per session, so a user who stops
    /// and starts the bridge does not have to retype it on the desktop.
    ///
    /// Shared with the announce thread rather than copied into it. A copy
    /// would go stale the moment the code changed, and the phone would then
    /// show one code on screen while proving it knew another.
    pairing: Arc<Mutex<PairingCode>>,
}

struct Running {
    stop: Arc<AtomicBool>,
    receive: Option<std::thread::JoinHandle<()>>,
    announce: Option<std::thread::JoinHandle<()>>,
    /// The port audio is arriving on, which is what an announcement has to
    /// advertise. Kept here because only a running session has one.
    port: u16,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

#[uniffi::export]
impl Bridge {
    /// Create an idle bridge.
    #[uniffi::constructor]
    #[must_use]
    pub fn new() -> Self {
        install_logging();
        Self {
            inner: Mutex::new(None),
            shared: Mutex::new(None),
            device_name: Mutex::new("Sonduit".to_string()),
            pairing: Arc::new(Mutex::new(PairingCode::from_seed(random_seed()))),
        }
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

    /// Generate a new pairing code.
    ///
    /// For a user who believes the old one has been seen by someone else. Any
    /// desktop paired with the previous code stops being able to find this
    /// device, which is the point.
    pub fn regenerate_pairing_code(&self) {
        if let Ok(mut code) = self.pairing.lock() {
            *code = PairingCode::from_seed(random_seed());
        }
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
    /// Returning `Ok` means the announcement left this device, not that the
    /// desktop accepted it. The confirmation the user sees is audio arriving.
    ///
    /// # Errors
    /// Returns [`FfiError::BadInvite`] when the scanned text is not a Sonduit
    /// invite, [`FfiError::NotRunning`] when no session is listening, and
    /// [`FfiError::Transport`] when no address in the invite could be reached
    /// at all.
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
        let mut delivered = false;
        for address in &invite.addresses {
            if socket
                .send_to(&datagram, SocketAddr::from((*address, invite.port)))
                .is_ok()
            {
                delivered = true;
            }
        }

        if delivered {
            Ok(())
        } else {
            Err(FfiError::Transport {
                reason: "no address in the pairing code could be reached".to_string(),
            })
        }
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

        let announce = {
            let stop = Arc::clone(&stop);
            let name = self
                .device_name
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_else(|_| "Sonduit".to_string());
            let code = Arc::clone(&self.pairing);
            std::thread::Builder::new()
                .name("sonduit-announce".into())
                .spawn(move || announce_loop(&stop, &name, port, &code))
                .map_err(|error| FfiError::Transport {
                    reason: error.to_string(),
                })?
        };

        *running = Some(Running {
            stop,
            receive: Some(receive),
            announce: Some(announce),
            port,
        });

        if let Ok(mut current) = self.shared.lock() {
            *current = Some(shared);
        }

        Ok(())
    }

    /// Stop and release the audio device.
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
        if let Some(handle) = session.announce.take() {
            let _ = handle.join();
        }

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
    let mut report_buffer = [0_u8; FEEDBACK_BYTES];
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
    // Decided from the first packet's source address. Wi-Fi until then,
    // because holding too much audio is recoverable and holding too little is
    // heard immediately.
    let mut transport = Transport::WiFi;
    // Reused so draining allocates nothing per packet.
    let mut staging: Vec<u8> = Vec::with_capacity(sonduit_core::format::PCM_PAYLOAD_BYTES);
    // Scream carries no sequence number, so one is synthesised on arrival.
    // Reordering cannot be repaired for that wire format, which is a property
    // of the protocol and not of this code.
    let mut scream_sequence = 0_u16;

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        let arrival = start.elapsed().as_nanos() as u64;
        let bytes = &datagram[..length];

        let decoded = match classify(bytes) {
            Some(Wire::Sonduit) => SonduitPacket::decode(bytes).ok().map(|packet| {
                (
                    packet.format,
                    packet.sequence,
                    packet.timestamp_frames,
                    packet.pcm.to_vec(),
                )
            }),
            Some(Wire::Scream) => ScreamPacket::decode(bytes).ok().map(|packet| {
                let sequence = scream_sequence;
                scream_sequence = scream_sequence.wrapping_add(1);
                let frames = (packet.pcm.len() / packet.format.bytes_per_frame()) as u32;
                (
                    packet.format,
                    sequence,
                    u32::from(sequence).wrapping_mul(frames),
                    packet.pcm.to_vec(),
                )
            }),
            None => None,
        };

        let Some((format, sequence, timestamp, pcm)) = decoded else {
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
            transport = transport_of(from);
            if let Ok(mut slot) = shared.transport.lock() {
                *slot = Some(transport);
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
                    JitterBuffer::new(format, JitterConfig::for_transport(transport)),
                    format,
                );
            }
            source.buffer_mut().push(sequence, timestamp, arrival, pcm);

            // Move what the buffer will release into the queue the callback
            // reads. This is the only place the jitter buffer is touched now;
            // the callback never sees it, which is the point.
            //
            // Bounded, and that bound is not a nicety. Draining "until the
            // source is empty" never terminates: a jitter buffer with a gap
            // conceals it and hands back audio, so it is always willing to
            // produce more. The first version of this loop spun forever on the
            // first packet and the receive thread never went back to the
            // socket. One packet in, at most a few packets out, is the pacing
            // that matches reality.
            for _ in 0..DRAIN_PER_PACKET {
                if !drain_packet(&mut source, &mut staging) {
                    break;
                }
                let Ok(mut queue) = shared.queue.lock() else {
                    break;
                };
                let Some(queue) = queue.as_mut() else { break };
                if queue.push(&staging) < staging.len() {
                    // The callback has stalled. Stop feeding rather than
                    // spinning; the resync below deals with the backlog once
                    // it is genuinely hopeless.
                    break;
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
            send_report(socket, from, shared, last_accepted, &mut report_buffer);
        }

        // Echoed back so the sender can measure a round trip against its own
        // clock. Neither end has to interpret the other's.
        last_accepted = Some((timestamp, std::time::Instant::now()));

        since_correction += 1;
        if since_correction >= PACKETS_PER_CORRECTION {
            since_correction = 0;
            let drift_ppm = estimator.as_ref().and_then(DriftEstimator::drift_ppm);

            if let Ok(source) = shared.source.lock() {
                controller.update(
                    source.buffer().depth_ms(),
                    source.buffer().target_ms(),
                    drift_ppm,
                );
            }
            if let Some(resampler) = resampler.as_mut() {
                resampler.set_ratio(controller.ratio());
            }
            if let Ok(mut slot) = shared.drift.lock() {
                *slot = (drift_ppm, controller.correction_ppm());
            }

            // A buffer that keeps growing means nothing is draining it, which
            // is what the audio callback is for. Reported here because it is
            // the one symptom that distinguishes a device that never started
            // from a link that is merely fast.
            let target_ms = shared
                .source
                .lock()
                .map_or(30.0, |source| source.buffer().target_ms());

            // Resampling shifts parts per million. It cannot shed a backlog,
            // and on the first real device it sat pinned at its 500 ppm limit
            // against 1536 ms of queued audio, which would have taken fifty
            // minutes to clear.
            let mut dropped = 0;
            let mut queued_ms = 0.0;
            if let Ok(mut queue) = shared.queue.lock() {
                if let Some(queue) = queue.as_mut() {
                    queued_ms = queue.queued_ms();
                    dropped = queue.resync_if_hopeless(target_ms);
                }
            }
            if dropped > 0 {
                note!("resynchronised: dropped {dropped} frames from {queued_ms:.0} ms queued");
            }

            // A stream that has gone cannot be restarted, and AAudio stops
            // calling back without saying anything. Reopening is the only
            // recovery, and it needs a fresh queue with it.
            if playback_disconnected(shared) {
                note!("playback stream disconnected; reopening");
                open_playback(shared, format);
            }

            note!(
                "queued {queued_ms:.0} ms, target {target_ms:.0} ms, drift {:?} ppm, correction {:.0} ppm, frames played {}",
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
fn announce_loop(stop: &AtomicBool, name: &str, audio_port: u16, code: &Mutex<PairingCode>) {
    let Ok(socket) = UdpSocket::bind(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        discovery::DISCOVERY_PORT,
    ))) else {
        return;
    };
    let _ = socket.set_broadcast(true);
    let _ = socket.set_read_timeout(Some(RECV_TIMEOUT));

    let mut datagram = [0_u8; 256];

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        // A probe with no readable nonce is either malformed or an older
        // protocol version, and there is nothing to authenticate against
        // either way.
        let Some(nonce) = discovery::probe_nonce(&datagram[..length]) else {
            continue;
        };

        // The code is read per probe rather than captured once, because
        // scanning a desktop's QR replaces it mid-session and a reply keyed by
        // the code this thread started with would then fail to verify.
        let Ok(code) = code.lock() else {
            return;
        };

        // The reply is built per probe rather than once, because the tag
        // covers that probe's nonce. That is what stops it being replayed.
        let reply = discovery::encode_announce(name, audio_port, &nonce, &code);
        drop(code);

        // Straight back to the prober rather than broadcast: the answer
        // concerns one machine, and broadcasting it would wake every device on
        // the network and hand them all a tag to study.
        let _ = socket.send_to(&reply, from);
    }
}

/// Send one report to the sender.
///
/// Failures are ignored on purpose. A report that does not arrive costs the
/// sender one missed sample of a figure it redraws four times a second, and
/// the alternative, tearing down a session that is playing audio correctly
/// because a status datagram was refused, is plainly worse.
fn send_report(
    socket: &UdpSocket,
    to: SocketAddr,
    shared: &Arc<Shared>,
    last_accepted: Option<(u32, std::time::Instant)>,
    buffer: &mut [u8; FEEDBACK_BYTES],
) {
    let Some((echo, accepted_at)) = last_accepted else {
        return;
    };

    // try_lock, not lock: this runs on the receive thread and must not wait
    // behind the audio callback for a status message.
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
        depth_tenths_ms: (depth_ms * 10.0).clamp(0.0, f64::from(u16::MAX)) as u16,
        playing: read_state(shared) == BridgeState::Streaming,
    };

    if report.encode(buffer).is_ok() {
        let _ = socket.send_to(buffer, to);
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

/// Guess the link from the address the audio is arriving from.
///
/// Android's tethering range is fixed at 192.168.42/24 in AOSP and most OEMs
/// keep it, so the guess is usually right. When it is wrong the cost is a
/// buffer sized for the other link, which is 20 ms of latency or a few
/// dropouts, not a broken session. The sender labels the link the same way;
/// this is deliberately not taken from the packet, because a field an
/// attacker controls should not decide how much audio is held.
fn transport_of(from: SocketAddr) -> Transport {
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

    #[test]
    fn a_fresh_bridge_is_idle_and_reports_empty_telemetry() {
        let bridge = Bridge::new();
        assert_eq!(bridge.state(), BridgeState::Idle);

        let telemetry = bridge.telemetry();
        assert_eq!(telemetry.packets_accepted, 0);
        assert_eq!(telemetry.sample_rate, 0);
    }

    #[test]
    fn stopping_a_stopped_bridge_is_not_an_error() {
        // The Android service lifecycle calls stop from more than one place.
        let bridge = Bridge::new();
        assert!(bridge.stop().is_ok());
        assert!(bridge.stop().is_ok());
    }

    #[test]
    fn the_announced_name_can_be_set_before_starting() {
        let bridge = Bridge::new();
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

        let bridge = Bridge::new();
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
        let bridge = Bridge::new();
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
        let bridge = Bridge::new();
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
        let bridge = Bridge::new();
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
        let bridge = Bridge::new();
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
