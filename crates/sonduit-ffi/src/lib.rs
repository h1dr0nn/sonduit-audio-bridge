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
use sonduit_core::jitter::{JitterBuffer, JitterConfig};
use sonduit_core::packet::{ScreamPacket, SonduitPacket};
use sonduit_core::ratio::{RatioConfig, RatioController};
use sonduit_core::resample::DriftResampler;
use sonduit_playback_android::{CallbackSource, JitterSource};
use sonduit_transport::{classify, discovery, Wire, DEFAULT_PORT};

uniffi::setup_scaffolding!();

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
}

/// State shared between the receive thread and the FFI callers.
struct Shared {
    source: Mutex<JitterSource>,
    state: Mutex<BridgeState>,
    malformed: Mutex<u64>,
    format: Mutex<Option<Format>>,
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
}

/// A handle the Android app holds for the lifetime of a session.
#[derive(uniffi::Object)]
pub struct Bridge {
    inner: Mutex<Option<Running>>,
    shared: Mutex<Option<Arc<Shared>>>,
    device_name: Mutex<String>,
}

struct Running {
    stop: Arc<AtomicBool>,
    receive: Option<std::thread::JoinHandle<()>>,
    announce: Option<std::thread::JoinHandle<()>>,
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
        Self {
            inner: Mutex::new(None),
            shared: Mutex::new(None),
            device_name: Mutex::new("Sonduit".to_string()),
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
            #[cfg(target_os = "android")]
            playback: Mutex::new(None),
            playback_error: Mutex::new(None),
            drift: Mutex::new((None, 0.0)),
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
            std::thread::Builder::new()
                .name("sonduit-announce".into())
                .spawn(move || announce_loop(&stop, &name, port))
                .map_err(|error| FfiError::Transport {
                    reason: error.to_string(),
                })?
        };

        *running = Some(Running {
            stop,
            receive: Some(receive),
            announce: Some(announce),
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

impl CallbackSource for Shared {
    fn fill(&self, out: &mut [i16], frames: usize) -> usize {
        // Delegates to the blanket implementation on the mutex, which does the
        // non-blocking acquire. Nothing else in this type is touched from the
        // audio thread.
        self.source.fill(out, frames)
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
    // Scream carries no sequence number, so one is synthesised on arrival.
    // Reordering cannot be repaired for that wire format, which is a property
    // of the protocol and not of this code.
    let mut scream_sequence = 0_u16;

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, _from)) = socket.recv_from(&mut datagram) else {
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
            estimator.observe(sender_frames, arrival);
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
                *source =
                    JitterSource::new(JitterBuffer::new(format, JitterConfig::default()), format);
            }
            source.buffer_mut().push(sequence, timestamp, arrival, pcm);
        }

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
            set_state(shared, BridgeState::Streaming);
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
    let source: Arc<dyn CallbackSource> = Arc::<Shared>::clone(shared) as Arc<dyn CallbackSource>;

    let Ok(mut slot) = shared.playback.lock() else {
        return;
    };
    if let Some(previous) = slot.take() {
        let _ = previous.stop();
    }

    match sonduit_playback_android::Playback::open(format, source) {
        Ok(playback) => {
            *slot = Some(playback);
            if let Ok(mut error) = shared.playback_error.lock() {
                *error = None;
            }
        }
        Err(error) => {
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
fn announce_loop(stop: &AtomicBool, name: &str, audio_port: u16) {
    let Ok(socket) = UdpSocket::bind(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        discovery::DISCOVERY_PORT,
    ))) else {
        return;
    };
    let _ = socket.set_broadcast(true);
    let _ = socket.set_read_timeout(Some(RECV_TIMEOUT));

    let reply = discovery::encode_announce(name, audio_port);
    let mut datagram = [0_u8; 256];

    while !stop.load(Ordering::Relaxed) {
        let Ok((length, from)) = socket.recv_from(&mut datagram) else {
            continue;
        };
        if discovery::peek_kind(&datagram[..length]) == Some(discovery::MessageKind::Probe) {
            // Reply straight back to the prober rather than broadcasting: the
            // answer concerns one machine, and broadcasting it would wake
            // every device on the network.
            let _ = socket.send_to(&reply, from);
        }
    }
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
}
