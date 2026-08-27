//! What the bridge reports, and how the numbers are derived.
//!
//! The shapes here are serialised straight into the webview, so the field
//! names are the ones the JavaScript reads. Keeping the derivation in Rust
//! rather than in the UI means one implementation of "what does latency mean",
//! and it can be tested.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::Serialize;
use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};

/// How often the accumulator produces a new view.
///
/// Matched to the emit interval. Recomputing more often would only average
/// over a shorter window and make the numbers jumpier.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// The format and destination of a running session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Name of the device being captured.
    pub endpoint: String,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u8,
    /// Bit depth on the wire.
    pub bit_depth: u8,
    /// Where audio is being sent.
    pub target: String,
    /// Which link this is, as far as the address can tell.
    pub transport: String,
    /// Wire format in use.
    pub wire: String,
}

impl SessionInfo {
    /// Describe a session that is about to start.
    #[must_use]
    pub fn new(endpoint: &str, format: Format, target: SocketAddr, scream: bool) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            sample_rate: format.sample_rate,
            channels: format.channels,
            bit_depth: format.bit_depth.bits(),
            target: target.to_string(),
            transport: classify_transport(target),
            wire: if scream { "scream" } else { "sonduit" }.to_string(),
        }
    }
}

/// Guess the link from the destination address.
///
/// This is a label, not a routing decision. Android's tethering range is fixed
/// at 192.168.42/24 in AOSP and most OEMs keep it, so the guess is usually
/// right; when it is wrong the only cost is a word in the UI.
fn classify_transport(target: SocketAddr) -> String {
    match target.ip() {
        std::net::IpAddr::V4(ip) if ip.is_multicast() => "multicast".to_string(),
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            if octets[0] == 192 && octets[1] == 168 && octets[2] == 42 {
                "usb".to_string()
            } else {
                "wifi".to_string()
            }
        }
        std::net::IpAddr::V6(_) => "wifi".to_string(),
    }
}

/// Numbers the UI displays.
///
/// Every field is optional because "not measured yet" and "measured as zero"
/// are different, and showing a confident 0.0 for something that has never
/// been sampled is a lie the user cannot detect.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryView {
    /// One-way latency the sender can account for, in milliseconds.
    pub latency_ms: Option<f64>,
    /// Audio held in the send path, in milliseconds.
    pub buffer_depth_ms: Option<f64>,
    /// Fraction of datagrams the socket refused, in percent.
    pub packet_loss_pct: Option<f64>,
    /// Clock drift, which only the receiver can measure.
    pub drift_ppm: Option<f64>,
    /// Inter-arrival jitter, likewise receiver-side.
    pub jitter_ms: Option<f64>,
    /// Packets that arrived too late to play, receiver-side.
    pub late_packets: Option<u64>,
    /// Packets that arrived out of order, receiver-side.
    pub reordered_packets: Option<u64>,
    /// Seconds since the session started.
    pub uptime_seconds: Option<u64>,
    /// Datagrams sent.
    pub packets_sent: Option<u64>,
    /// Audio actually captured, in seconds.
    pub audio_seconds: Option<f64>,
}

/// A complete view of the bridge, as the webview receives it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSnapshot {
    /// True once the backend can drive a session at all.
    pub available: bool,
    /// One of `disconnected`, `connecting`, `connected`, `error`.
    pub status: String,
    /// Present while a session is configured.
    pub session: Option<SessionInfo>,
    /// Current readings.
    pub telemetry: TelemetryView,
    /// Last failure, if any. Cleared when a session starts.
    pub error: Option<String>,
}

impl Default for BridgeSnapshot {
    fn default() -> Self {
        Self {
            // The backend exists as soon as this type does. Whether capture
            // will succeed is a separate question, answered by starting.
            available: cfg!(windows),
            status: "disconnected".to_string(),
            session: None,
            telemetry: TelemetryView::default(),
            error: None,
        }
    }
}

impl BridgeSnapshot {
    /// A snapshot for a session that has been configured but has sent nothing.
    #[must_use]
    pub fn starting(session: SessionInfo) -> Self {
        Self {
            available: true,
            status: "connecting".to_string(),
            session: Some(session),
            telemetry: TelemetryView::default(),
            error: None,
        }
    }

    /// Fold a fresh reading in.
    ///
    /// A session that has sent packets is reported as connected: over UDP
    /// there is nothing else to go on, because the receiver never
    /// acknowledges. This is honest about what the sender can know.
    pub fn apply(&mut self, view: TelemetryView) {
        if view.packets_sent.unwrap_or(0) > 0 {
            self.status = "connected".to_string();
        }
        self.telemetry = view;
    }

    /// Record that the session has ended.
    pub fn mark_stopped(&mut self) {
        self.status = "disconnected".to_string();
        self.session = None;
        self.telemetry = TelemetryView::default();
    }

    /// Report that the session is broken and why.
    pub fn mark_error(&mut self, message: &str) {
        self.status = "error".to_string();
        self.error = Some(message.to_string());
    }

    /// Attach the most recent failure without claiming the session is broken.
    ///
    /// A transient send failure explains a non-zero loss figure; it does not
    /// mean the bridge has stopped working, and showing it as an error would
    /// train the user to ignore the one that matters.
    pub fn note_error(&mut self, message: Option<&str>) {
        if self.status != "error" {
            self.error = message.map(ToString::to_string);
        }
    }
}

/// Collects counts on the audio thread and turns them into a view on a timer.
///
/// Deliberately not atomic and not shared: it lives on the capture thread and
/// only the finished view crosses a lock. Contending on a counter inside the
/// audio loop is how glitches get introduced.
pub struct Accumulator {
    format: Format,
    started: Instant,
    last_sample: Instant,
    frames: u64,
    packets: u64,
    send_failures: u64,
    capture_failures: u64,
    last_error: Option<String>,
}

impl Accumulator {
    /// Start counting for a session in `format`.
    #[must_use]
    pub fn new(format: Format) -> Self {
        let now = Instant::now();
        Self {
            format,
            started: now,
            last_sample: now,
            frames: 0,
            packets: 0,
            send_failures: 0,
            capture_failures: 0,
            last_error: None,
        }
    }

    /// Record a successful block.
    pub fn record_sent(&mut self, frames: usize, packets_total: u64) {
        self.frames += frames as u64;
        self.packets = packets_total;
    }

    /// Record a datagram the socket refused.
    pub fn record_send_error(&mut self, message: &str) {
        self.send_failures += 1;
        self.last_error = Some(message.to_string());
    }

    /// Record a failed read from the capture device.
    pub fn record_capture_error(&mut self, message: &str) {
        self.capture_failures += 1;
        self.last_error = Some(message.to_string());
    }

    /// The most recent error, if one has happened.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// A view, if enough time has passed since the last one.
    pub fn due(&mut self) -> Option<TelemetryView> {
        let now = Instant::now();
        if now.duration_since(self.last_sample) < SAMPLE_INTERVAL {
            return None;
        }
        self.last_sample = now;
        Some(self.view(now))
    }

    /// A view regardless of timing, for tests and for the final snapshot.
    #[must_use]
    pub fn view_now(&self) -> TelemetryView {
        self.view(Instant::now())
    }

    fn view(&self, now: Instant) -> TelemetryView {
        let attempted = self.packets + self.send_failures;
        let loss = if attempted == 0 {
            0.0
        } else {
            self.send_failures as f64 * 100.0 / attempted as f64
        };

        TelemetryView {
            // The sender can only account for its own share of the budget:
            // one capture period plus one packet of buffering. Everything
            // downstream is the receiver's to report, and claiming a
            // round-trip figure the sender cannot see would be a fiction.
            latency_ms: Some(self.send_side_latency_ms()),
            buffer_depth_ms: Some(self.packet_duration_ms()),
            packet_loss_pct: Some(loss),
            // Receiver-side measurements. None, not zero: the sender has no
            // way to know these and must not pretend otherwise.
            drift_ppm: None,
            jitter_ms: None,
            late_packets: None,
            reordered_packets: None,
            uptime_seconds: Some(now.duration_since(self.started).as_secs()),
            packets_sent: Some(self.packets),
            audio_seconds: Some(self.frames as f64 / f64::from(self.format.sample_rate)),
        }
    }

    fn packet_duration_ms(&self) -> f64 {
        let frames = PCM_PAYLOAD_BYTES / self.format.bytes_per_frame();
        frames as f64 * 1000.0 / f64::from(self.format.sample_rate)
    }

    fn send_side_latency_ms(&self) -> f64 {
        // The engine period, plus the time it takes to fill one packet. See
        // stages 1 to 4 of docs/latency-budget.md.
        f64::from(crate::bridge::CAPTURE_PERIOD_MS) + self.packet_duration_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulator() -> Accumulator {
        Accumulator::new(Format::stereo_48k())
    }

    #[test]
    fn an_untouched_session_reports_no_loss_rather_than_a_nan() {
        // Zero over zero would render as NaN in the UI.
        let view = accumulator().view_now();
        assert_eq!(view.packet_loss_pct, Some(0.0));
        assert_eq!(view.packets_sent, Some(0));
    }

    #[test]
    fn loss_is_the_share_of_datagrams_the_socket_refused() {
        let mut counters = accumulator();
        counters.record_sent(480, 3);
        counters.record_send_error("network unreachable");

        let view = counters.view_now();
        assert_eq!(view.packet_loss_pct, Some(25.0));
    }

    #[test]
    fn receiver_side_readings_are_absent_rather_than_zero() {
        // The sender cannot measure drift or jitter; reporting 0.0 would tell
        // the user the clocks are perfect when nothing has been measured.
        let mut counters = accumulator();
        counters.record_sent(4_800, 10);

        let view = counters.view_now();
        assert_eq!(view.drift_ppm, None);
        assert_eq!(view.jitter_ms, None);
        assert_eq!(view.late_packets, None);
        assert_eq!(view.reordered_packets, None);
    }

    #[test]
    fn audio_seconds_follows_the_frame_count_and_the_rate() {
        let mut counters = accumulator();
        counters.record_sent(48_000, 1);

        let view = counters.view_now();
        assert_eq!(view.audio_seconds, Some(1.0));
    }

    #[test]
    fn the_reported_latency_is_the_period_plus_one_packet() {
        // 1152 bytes at 48 kHz stereo 16-bit is 288 frames, which is 6 ms.
        let view = accumulator().view_now();
        assert_eq!(view.buffer_depth_ms, Some(6.0));
        assert_eq!(view.latency_ms, Some(16.0));
    }

    #[test]
    fn a_view_is_not_produced_again_immediately() {
        // The emit timer, not the audio loop, sets the reporting rate.
        let mut counters = accumulator();
        counters.record_sent(480, 1);
        assert!(counters.due().is_none(), "not yet a quarter of a second");
    }

    #[test]
    fn a_snapshot_becomes_connected_once_a_packet_has_gone_out() {
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            false,
        ));
        assert_eq!(snapshot.status, "connecting");

        snapshot.apply(TelemetryView {
            packets_sent: Some(1),
            ..TelemetryView::default()
        });
        assert_eq!(snapshot.status, "connected");
    }

    #[test]
    fn a_session_with_no_packets_stays_connecting() {
        // Over UDP nothing acknowledges, so "packets left the machine" is the
        // only evidence there is. Zero packets is not evidence.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            false,
        ));
        snapshot.apply(TelemetryView {
            packets_sent: Some(0),
            ..TelemetryView::default()
        });
        assert_eq!(snapshot.status, "connecting");
    }

    #[test]
    fn stopping_clears_the_session_and_the_readings() {
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            false,
        ));
        snapshot.apply(TelemetryView {
            packets_sent: Some(100),
            ..TelemetryView::default()
        });

        snapshot.mark_stopped();

        assert_eq!(snapshot.status, "disconnected");
        assert!(snapshot.session.is_none());
        // Stale numbers next to a disconnected pill read as a live session.
        assert_eq!(snapshot.telemetry, TelemetryView::default());
    }

    #[test]
    fn a_transient_failure_is_shown_without_calling_the_session_broken() {
        // Loss on WiFi is normal. Reporting every dropped datagram as an error
        // would train the user to ignore the status entirely.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            false,
        ));
        snapshot.apply(TelemetryView {
            packets_sent: Some(50),
            ..TelemetryView::default()
        });
        snapshot.note_error(Some("network unreachable"));

        assert_eq!(snapshot.status, "connected");
        assert_eq!(snapshot.error.as_deref(), Some("network unreachable"));
    }

    #[test]
    fn a_broken_session_is_not_talked_back_into_working_by_a_later_note() {
        let mut snapshot = BridgeSnapshot::default();
        snapshot.mark_error("the capture device disappeared");
        snapshot.note_error(Some("something less serious"));

        assert_eq!(snapshot.status, "error");
        assert_eq!(
            snapshot.error.as_deref(),
            Some("the capture device disappeared"),
            "the real reason must survive"
        );
    }

    #[test]
    fn the_tethering_range_is_labelled_usb() {
        let usb = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.42.129:4010".parse().unwrap(),
            false,
        );
        assert_eq!(usb.transport, "usb");
    }

    #[test]
    fn an_ordinary_lan_address_is_labelled_wifi() {
        let wifi = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            false,
        );
        assert_eq!(wifi.transport, "wifi");
    }

    #[test]
    fn the_multicast_group_is_labelled_as_multicast_not_guessed_at() {
        let group = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "239.255.77.77:4010".parse().unwrap(),
            false,
        );
        assert_eq!(group.transport, "multicast");
    }

    #[test]
    fn the_session_carries_the_format_that_will_actually_be_sent() {
        let session = SessionInfo::new(
            "Headset",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            true,
        );
        assert_eq!(session.sample_rate, 48_000);
        assert_eq!(session.channels, 2);
        assert_eq!(session.bit_depth, 16);
        assert_eq!(session.wire, "scream");
    }
}
