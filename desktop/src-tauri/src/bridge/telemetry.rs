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
use sonduit_transport::feedback::{end_to_end_ms, one_way_ms, Feedback};

use crate::bridge::link::LinkKind;

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
    /// Where audio is being sent. Follows the session across a migration.
    pub target: String,
    /// Which link this is.
    ///
    /// Not derived here and not guessed at. It is
    /// [`LinkKind::label`] of the link the send loop is actually using, which
    /// is the same value the packet header carries as `FLAG_WIRED_LINK`. The
    /// panel used to test the target against 192.168.42/24 instead, and said
    /// "Wi-Fi" for a phone tethering on 10.114.89.x while the audio went over
    /// the cable.
    pub transport: String,
    /// Wire format in use.
    pub wire: String,
    /// Whether every datagram of this session is encrypted.
    ///
    /// Not a guess and not a setting: it is whether the send loop was given a
    /// sealer, which is the same thing as whether the packets going out carry
    /// version 2 and a Poly1305 tag. The panel shows it because a session that
    /// is not encrypted must never look like one that is, and the one way to
    /// get an unencrypted session is Scream compatibility, which cannot be
    /// encrypted at all. See ADR-009.
    pub encrypted: bool,
}

impl SessionInfo {
    /// Describe a session that is about to start.
    #[must_use]
    pub fn new(
        endpoint: &str,
        format: Format,
        target: SocketAddr,
        link: LinkKind,
        scream: bool,
        encrypted: bool,
    ) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            sample_rate: format.sample_rate,
            channels: format.channels,
            bit_depth: format.bit_depth.bits(),
            target: target.to_string(),
            transport: link.label().to_string(),
            wire: if scream { "scream" } else { "sonduit" }.to_string(),
            encrypted,
        }
    }

    /// Point the session at a different route, after a migration.
    pub fn moved_to(&mut self, target: SocketAddr, link: LinkKind) {
        self.target = target.to_string();
        self.transport = link.label().to_string();
    }

    /// Name the endpoint the session is now tapping, after a reopen.
    pub fn captured_from(&mut self, endpoint: &str) {
        self.endpoint = endpoint.to_string();
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
    /// Capture to ear, as far as both ends can account for it.
    ///
    /// `None` until a receiver has answered. Before that nothing about the
    /// network or the far end has been measured, and the sender's own share is
    /// a property of its configuration rather than a latency anyone hears.
    pub latency_ms: Option<f64>,
    /// Audio the receiver is holding, in milliseconds, as it reported.
    pub buffer_depth_ms: Option<f64>,
    /// Packets that never reached the receiver, in percent, as it counted.
    ///
    /// Not the sender's own count of refused datagrams: a socket that accepted
    /// everything says nothing about what arrived.
    pub packet_loss_pct: Option<f64>,
    /// Measured round trip to the receiver, in milliseconds.
    pub round_trip_ms: Option<f64>,
    /// Datagrams the receiver accepted.
    pub packets_received: Option<u64>,
    /// Seconds since the receiver was last heard from.
    ///
    /// `None` when one has never answered.
    pub last_heard_seconds: Option<f64>,
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
    /// Reports that arrived and did not authenticate.
    ///
    /// `None` for a session with no key, where there is nothing to
    /// authenticate against and a zero would be a claim there was. On a keyed
    /// session on a healthy network this is zero, and anything else is either
    /// a bug or somebody on the network sending forged status datagrams. It is
    /// shown for that reason: it is the only evidence of the second.
    pub refused_reports: Option<u64>,
    /// Datagrams the socket refused, over the whole session.
    ///
    /// The record of every send failure, including the ones that healed
    /// before anybody could see them. [`Accumulator::last_error`] retires a
    /// message the moment a datagram gets through, which is the honest thing
    /// to say about a fault that is over; this is what stops that being the
    /// same as pretending it never happened. Zero on a healthy session.
    pub send_failures: Option<u64>,
    /// Reads the capture device refused, over the whole session.
    ///
    /// Counted separately from [`Self::send_failures`] and never added to it:
    /// one is the audio device and the other is the network, they recover in
    /// completely different ways, and a single total would say neither.
    pub capture_failures: Option<u64>,
}

/// What has to happen before a recorded failure stops being true.
///
/// A failure is not news for as long as it lasts and not news once it is
/// over; the only question is which success ends it. A socket that refuses a
/// datagram is answered by a datagram getting through, and a capture device
/// that refuses a read is answered by a read succeeding. Neither answers the
/// other: audio still arriving from the sound card says nothing about whether
/// the network came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    /// A block of datagrams leaving the machine.
    Sent,
    /// A block read from the capture device.
    Read,
}

/// A failure, and what it is waiting to be superseded by.
#[derive(Debug, Clone)]
struct Fault {
    /// What to tell the user while it is still true.
    message: String,
    /// Which counter ends it.
    recovery: Recovery,
    /// That counter's value when this happened. While it has not moved,
    /// nothing of the kind that would disprove this fault has happened.
    at: u64,
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
    /// Connected means a receiver has answered, and nothing else. Sending
    /// datagrams into a multicast group nobody has joined is not a connection,
    /// and reporting it as one was the bug this whole feedback path exists to
    /// fix: the bridge said "connected, 16 ms, 0% loss" with no device on the
    /// network at all.
    pub fn apply(&mut self, view: TelemetryView) {
        if self.status != "error" {
            self.status = if view.last_heard_seconds.is_some() && view.latency_ms.is_some() {
                "connected"
            } else {
                "connecting"
            }
            .to_string();
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

    /// Clear a fault that has resolved.
    ///
    /// A status pinned on an error the user has already fixed, by plugging the
    /// headset back in, is worse than no status: it says the bridge is broken
    /// while audio is playing.
    pub fn clear_error(&mut self) {
        if self.status == "error" {
            self.status = if self.telemetry.packets_sent.unwrap_or(0) > 0 {
                "connected".to_string()
            } else {
                "connecting".to_string()
            };
        }
        self.error = None;
    }

    /// Record that the session has moved onto a different link.
    ///
    /// The whole of what the user is told, and the whole of what the wire is
    /// told, come from the same value: the frontend watches `transport` change
    /// and announces it, and the packetizer sets its header flag from the same
    /// [`LinkKind`]. There is no second derivation to disagree with this one.
    pub fn note_link(&mut self, target: SocketAddr, link: LinkKind) {
        if let Some(session) = self.session.as_mut() {
            session.moved_to(target, link);
        }
    }

    /// Record that capture has moved to a different output device.
    ///
    /// Reopening after an unplugged headset can land on a different endpoint
    /// from the one the session started on, and a panel still naming the
    /// device that has gone is telling the user the audio is somewhere it is
    /// not. Same reasoning as [`BridgeSnapshot::note_link`]: what is displayed
    /// comes from what actually happened, never from what was asked for.
    pub fn note_endpoint(&mut self, endpoint: &str) {
        if let Some(session) = self.session.as_mut() {
            session.captured_from(endpoint);
        }
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
    reopens: u64,
    /// Feedback datagrams refused because they did not authenticate.
    ///
    /// `None` on a session with no key: nothing is being checked, so there is
    /// nothing to count. Zero would say a check is running and passing.
    refused_reports: Option<u64>,
    /// Capture blocks whose datagrams all left the machine.
    ///
    /// Only ever compared against itself, so it does not matter that a block
    /// is a different amount of work from one session to the next: what is
    /// being asked is "has anything gone out since that failure", and any
    /// monotonic count of successes answers it.
    sends: u64,
    /// Blocks read from the capture device without error.
    reads: u64,
    /// The failure that is currently true, if one is.
    last_error: Option<Fault>,
    /// The receiver's most recent report, and when it arrived.
    ///
    /// Everything about the far end and the network comes from here. Nothing
    /// is displayed about either until one has been received.
    latest: Option<(Feedback, Instant)>,
    /// Smoothed round trip, from the estimator the send loop drives.
    round_trip_ms: Option<f64>,
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
            reopens: 0,
            refused_reports: None,
            sends: 0,
            reads: 0,
            last_error: None,
            latest: None,
            round_trip_ms: None,
        }
    }

    /// Record a successful block.
    pub fn record_sent(&mut self, frames: usize, packets_total: u64) {
        self.frames += frames as u64;
        self.packets = packets_total;
        self.sends += 1;
    }

    /// Record a block read from the capture device without error.
    ///
    /// Separate from [`Self::record_sent`] because a block can be read and
    /// then fail to send, and because a silent moment reads frames that never
    /// become a datagram. Both are the capture device working.
    pub fn record_read(&mut self) {
        self.reads += 1;
    }

    /// Record a datagram the socket refused.
    pub fn record_send_error(&mut self, message: &str) {
        self.send_failures += 1;
        self.last_error = Some(Fault {
            message: message.to_string(),
            recovery: Recovery::Sent,
            at: self.sends,
        });
    }

    /// Declare that this session's reports are authenticated.
    ///
    /// Turns the refused count from "not applicable" into a running total that
    /// starts at zero, which is the number a healthy keyed session shows. The
    /// distinction matters: a blank means nothing is being checked, and a zero
    /// means something is and has never failed.
    pub fn checking_reports(&mut self) {
        self.refused_reports.get_or_insert(0);
    }

    /// Record a report that did not authenticate.
    ///
    /// Deliberately not `record_send_error`: a forged report is not a fault in
    /// this machine or this link, and putting it on the status line would tell
    /// the user their bridge is broken when what happened is that somebody
    /// else's datagram was refused exactly as it should have been.
    pub fn record_refused_report(&mut self) {
        *self.refused_reports.get_or_insert(0) += 1;
    }

    /// Record a failed read from the capture device.
    pub fn record_capture_error(&mut self, message: &str) {
        self.capture_failures += 1;
        self.last_error = Some(Fault {
            message: message.to_string(),
            recovery: Recovery::Read,
            at: self.reads,
        });
    }

    /// Fold in a report from the receiver.
    pub fn record_feedback(&mut self, feedback: Feedback, round_trip_ms: Option<f64>) {
        self.latest = Some((feedback, Instant::now()));
        if round_trip_ms.is_some() {
            self.round_trip_ms = round_trip_ms;
        }
    }

    /// Whether a receiver has answered recently enough to still be there.
    ///
    /// Three missed reports, not one: a single dropped datagram is what this
    /// transport is built to expect, and treating it as a disconnection would
    /// make the status flicker on every busy moment.
    #[must_use]
    pub fn receiver_present(&self) -> bool {
        self.latest.is_some_and(|(_, at)| {
            at.elapsed() < Duration::from_millis(sonduit_transport::feedback::FEEDBACK_TIMEOUT_MS)
        })
    }

    /// Record that the capture device was replaced after failing.
    pub fn record_reopen(&mut self) {
        self.reopens += 1;
        self.last_error = None;
    }

    /// How many times the capture device has been replaced this session.
    ///
    /// A session that reopens repeatedly is a session with a real problem, and
    /// the count is the only evidence of it once each one has recovered.
    #[must_use]
    pub const fn reopens(&self) -> u64 {
        self.reopens
    }

    /// The failure that is still true, if one is.
    ///
    /// # The rule
    ///
    /// A failure is reported for exactly as long as nothing has disproved it,
    /// and not one tick longer. A send failure is disproved by a datagram
    /// getting through; a capture failure is disproved by a read succeeding.
    /// Until that happens the message stands, however many telemetry ticks
    /// that takes; the moment it happens the message is gone, without waiting
    /// for a timer and without anything having to remember to call a clearer.
    ///
    /// # Why it is not simply cleared somewhere
    ///
    /// It was, once, from the capture-reopen path and nowhere else, and the
    /// send path had no equivalent at all. A session that retreated from a
    /// pulled cable onto Wi-Fi went on reporting
    /// `socket error ... (os error 10049)` for sixty-six seconds after it had
    /// migrated back onto the cable and was demonstrably sending, because
    /// nothing on that path had a reason to call the clearer. Deriving the
    /// answer from the counters instead means there is no path that can
    /// forget: a fault ends because the thing that would end it happened, not
    /// because somebody remembered to say so.
    ///
    /// # What this does to the three cases
    ///
    /// A **transient** failure -- one datagram refused, the next accepted --
    /// is true for one capture block, about ten milliseconds. The telemetry
    /// tick is four times a second, so it is usually never published at all,
    /// and when a tick does land inside that window it is published for that
    /// one tick, which is true. A **persistent** failure is not suppressed by
    /// any of this: while every send is refused, `sends` never moves and the
    /// message stands on every tick until it does. And an **unseen** failure
    /// is not thrown away: every one of them is counted, and
    /// [`TelemetryView::send_failures`] and
    /// [`TelemetryView::capture_failures`] carry those counts to the user
    /// whether or not the message was ever on screen.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        let fault = self.last_error.as_ref()?;
        let succeeded = match fault.recovery {
            Recovery::Sent => self.sends,
            Recovery::Read => self.reads,
        };
        (succeeded == fault.at).then_some(fault.message.as_str())
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
        // Everything about the far end comes from the far end. A report that
        // has stopped arriving is not a report of zero.
        let present = self.receiver_present();
        let report = if present {
            self.latest.map(|(f, _)| f)
        } else {
            None
        };

        TelemetryView {
            latency_ms: report.and_then(|report| {
                self.round_trip_ms.map(|round_trip| {
                    end_to_end_ms(
                        self.send_side_latency_ms(),
                        one_way_ms(round_trip, report.hold_ms),
                        // Everything the receiver holds, not the jitter buffer
                        // alone. The hand-off queue behind it is audio the
                        // listener waits through exactly the same way, and a
                        // measured session carried more of it than of anything
                        // else on this line.
                        report.held_ms(),
                    )
                })
            }),
            buffer_depth_ms: report.map(|report| report.depth_ms()),
            packet_loss_pct: report.map(|report| report.loss_percent()),
            round_trip_ms: if present { self.round_trip_ms } else { None },
            packets_received: report.map(|report| report.accepted),
            last_heard_seconds: self.latest.map(|(_, at)| at.elapsed().as_secs_f64()),
            // Still receiver-side and still not reported over the wire. None,
            // not zero: a zero here would read as "the clocks match".
            drift_ppm: None,
            jitter_ms: None,
            late_packets: None,
            reordered_packets: None,
            uptime_seconds: Some(now.duration_since(self.started).as_secs()),
            packets_sent: Some(self.packets),
            audio_seconds: Some(self.frames as f64 / f64::from(self.format.sample_rate)),
            refused_reports: self.refused_reports,
            // Always a number, never a blank: a send is either attempted or
            // the session is not running, so zero here means "none refused"
            // rather than "nothing was counted". That is the opposite of
            // `refused_reports` above, where a blank is the honest answer for
            // a session with no key to check anything against.
            send_failures: Some(self.send_failures),
            capture_failures: Some(self.capture_failures),
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
    fn a_session_nobody_has_answered_reports_nothing_about_the_far_end() {
        // Not zero. Zero loss and zero latency are claims, and neither has
        // been measured until a receiver has said something.
        let view = accumulator().view_now();

        assert_eq!(view.packet_loss_pct, None);
        assert_eq!(view.latency_ms, None);
        assert_eq!(view.buffer_depth_ms, None);
        assert_eq!(view.round_trip_ms, None);
        assert_eq!(view.last_heard_seconds, None);
        // What the sender genuinely knows about itself is still reported.
        assert_eq!(view.packets_sent, Some(0));
    }

    #[test]
    fn a_report_is_what_makes_the_far_end_figures_appear() {
        let mut counters = accumulator();
        counters.record_sent(288, 1);
        counters.record_feedback(
            Feedback {
                echo: 288,
                hold_ms: 2,
                accepted: 100,
                lost: 4,
                depth_tenths_ms: 284,
                queue_tenths_ms: Some(120),
                playing: true,
            },
            Some(9.0),
        );

        let view = counters.view_now();
        assert_eq!(view.buffer_depth_ms, Some(28.4));
        assert_eq!(view.packets_received, Some(100));
        assert!(view
            .packet_loss_pct
            .is_some_and(|loss| (loss - 3.846).abs() < 0.01));
        assert_eq!(view.round_trip_ms, Some(9.0));
        // Send side 16 ms, one way (9 - 2) / 2 = 3.5, receiver holding 28.4 in
        // its jitter buffer and 12.0 more in the hand-off queue behind it.
        // Both are latency; counting only the first is what made a measured
        // session read 110 ms lower than it was.
        assert!(view.latency_ms.is_some_and(|ms| (ms - 59.9).abs() < 0.01));
    }

    #[test]
    fn a_refused_datagram_is_not_reported_as_packet_loss() {
        // It is a fault on this machine, not a measurement of the link, and
        // reporting it as loss put a number in front of the user that had
        // nothing to do with what the receiver got.
        let mut counters = accumulator();
        counters.record_sent(480, 3);
        counters.record_send_error("network unreachable");

        let view = counters.view_now();
        assert_eq!(view.packet_loss_pct, None);
        assert_eq!(counters.last_error(), Some("network unreachable"));
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
    fn the_send_side_share_is_the_period_plus_one_packet() {
        // 1152 bytes at 48 kHz stereo 16-bit is 288 frames, which is 6 ms, on
        // top of the 10 ms capture period. This is the sender's own
        // contribution and it is never displayed on its own: it only appears
        // once a receiver has reported the rest of the path.
        assert_eq!(accumulator().send_side_latency_ms(), 16.0);
        assert_eq!(accumulator().view_now().latency_ms, None);
    }

    #[test]
    fn a_view_is_not_produced_again_immediately() {
        // The emit timer, not the audio loop, sets the reporting rate.
        let mut counters = accumulator();
        counters.record_sent(480, 1);
        assert!(counters.due().is_none(), "not yet a quarter of a second");
    }
    /// A view of the kind the accumulator produces once a receiver has
    /// answered: it has heard from the far end and can account for a latency.
    fn answered() -> TelemetryView {
        TelemetryView {
            packets_sent: Some(500),
            last_heard_seconds: Some(0.05),
            latency_ms: Some(41.0),
            round_trip_ms: Some(8.0),
            packets_received: Some(498),
            buffer_depth_ms: Some(28.4),
            packet_loss_pct: Some(0.4),
            ..TelemetryView::default()
        }
    }

    #[test]
    fn a_snapshot_becomes_connected_once_a_receiver_answers() {
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));
        assert_eq!(snapshot.status, "connecting");

        snapshot.apply(answered());
        assert_eq!(snapshot.status, "connected");
    }

    #[test]
    fn sending_into_an_empty_network_is_not_a_connection() {
        // The bug this whole feedback path exists to fix. Multicast to a group
        // nobody has joined succeeds at every layer the sender can see, and it
        // was reported as a working session at 16 ms with no loss.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "239.255.77.77:4010".parse().unwrap(),
            LinkKind::Multicast,
            false,
            true,
        ));

        snapshot.apply(TelemetryView {
            packets_sent: Some(50_000),
            ..TelemetryView::default()
        });

        assert_eq!(snapshot.status, "connecting");
        assert_eq!(
            snapshot.telemetry.latency_ms, None,
            "a latency was invented"
        );
        assert_eq!(snapshot.telemetry.packet_loss_pct, None);
    }

    #[test]
    fn a_receiver_that_stops_answering_stops_being_connected() {
        // The phone was switched off, or walked out of range. The sender keeps
        // sending happily, and must not keep claiming a connection.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));
        snapshot.apply(answered());
        assert_eq!(snapshot.status, "connected");

        snapshot.apply(TelemetryView {
            packets_sent: Some(60_000),
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
            LinkKind::Wireless,
            false,
            true,
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
    fn a_resolved_fault_stops_being_reported() {
        // A headset plugged back in must not leave the status reading broken
        // while audio is playing.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));
        snapshot.apply(TelemetryView {
            packets_sent: Some(100),
            ..TelemetryView::default()
        });
        snapshot.mark_error("the capture device disappeared");
        assert_eq!(snapshot.status, "error");

        snapshot.clear_error();

        assert_eq!(snapshot.status, "connected");
        assert!(snapshot.error.is_none());
    }

    #[test]
    fn clearing_a_fault_before_anything_was_sent_reports_connecting() {
        // Recovered, but with no evidence yet that audio is reaching anyone.
        let mut snapshot = BridgeSnapshot::default();
        snapshot.mark_error("the capture device disappeared");
        snapshot.clear_error();
        assert_eq!(snapshot.status, "connecting");
    }

    #[test]
    fn a_send_failure_stops_being_reported_once_a_datagram_gets_through() {
        // The sixty-six second lie. A session retreated from a pulled cable
        // onto Wi-Fi, migrated back onto the cable, and went on displaying the
        // socket error from the moment the cable came out for the rest of its
        // life, while audio was arriving at the phone the whole time.
        let mut counters = accumulator();
        counters.record_sent(288, 1);
        counters.record_send_error(
            "socket error: The requested address is not valid in its context. (os error 10049)",
        );
        assert!(
            counters.last_error().is_some(),
            "a failure that has just happened is the truth about this session"
        );

        counters.record_sent(288, 2);

        assert_eq!(
            counters.last_error(),
            None,
            "a session that is sending is not a session that is broken"
        );
    }

    #[test]
    fn a_failure_that_is_still_happening_is_not_suppressed() {
        // The other half of the rule, and the one it would be dangerous to
        // get wrong: nothing here may quieten a fault that is still true.
        // Twenty telemetry ticks' worth of refusals with no send in between.
        let mut counters = accumulator();
        counters.record_sent(288, 1);
        for _ in 0..20 {
            counters.record_send_error("network is down");
            assert_eq!(counters.last_error(), Some("network is down"));
        }
    }

    #[test]
    fn a_failure_that_healed_unseen_is_still_counted() {
        // A transient refusal between two telemetry ticks is not worth a
        // message: by the time anybody could read it, it is no longer true.
        // It is still worth a number, because "this link refused four hundred
        // datagrams and recovered from every one" is a thing about the
        // session the user can act on and cannot otherwise find out.
        let mut counters = accumulator();
        counters.record_sent(288, 1);
        counters.record_send_error("no buffer space available");
        counters.record_sent(288, 2);
        counters.record_send_error("no buffer space available");
        counters.record_sent(288, 3);

        assert_eq!(counters.last_error(), None);
        let view = counters.view_now();
        assert_eq!(view.send_failures, Some(2));
        assert_eq!(view.capture_failures, Some(0));
    }

    #[test]
    fn audio_still_arriving_does_not_prove_the_network_came_back() {
        // The two faults are not interchangeable and neither retires the
        // other. A capture device that is happily handing over blocks says
        // nothing at all about a socket that is refusing them.
        let mut counters = accumulator();
        counters.record_send_error("network unreachable");
        for _ in 0..50 {
            counters.record_read();
        }
        assert_eq!(counters.last_error(), Some("network unreachable"));

        // And the reverse: a datagram going out says nothing about a capture
        // device that has been pulled.
        let mut counters = accumulator();
        counters.record_capture_error("device invalidated");
        counters.record_sent(288, 1);
        assert_eq!(counters.last_error(), Some("device invalidated"));
    }

    #[test]
    fn a_capture_failure_stops_being_reported_once_a_block_is_read() {
        // The same rule on the other path. Before this, the reopen path
        // called a clearer on the snapshot and the very next telemetry tick
        // put the message straight back, because the accumulator still held
        // it and republished it four times a second.
        let mut counters = accumulator();
        counters.record_capture_error("the capture device disappeared");
        assert!(counters.last_error().is_some());

        counters.record_read();

        assert_eq!(counters.last_error(), None);
    }

    #[test]
    fn a_reopen_clears_the_error_it_recovered_from() {
        let mut counters = accumulator();
        counters.record_capture_error("device invalidated");
        assert!(counters.last_error().is_some());

        counters.record_reopen();

        assert_eq!(counters.reopens(), 1);
        assert!(
            counters.last_error().is_none(),
            "the error that was recovered from is still being reported"
        );
    }

    #[test]
    fn a_reopen_onto_another_device_renames_the_endpoint_on_screen() {
        // The panel is the only place the user learns that the headset they
        // chose has gone and the audio is now coming off the speakers. A name
        // left pointing at the device that was unplugged says the opposite of
        // what happened.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Headset Earphone (Motorola Headset)",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));

        snapshot.note_endpoint("DELL U2419H (HD Audio Driver for Display Audio)");

        assert_eq!(
            snapshot
                .session
                .as_ref()
                .map(|session| session.endpoint.as_str()),
            Some("DELL U2419H (HD Audio Driver for Display Audio)")
        );
    }

    #[test]
    fn renaming_the_endpoint_of_a_stopped_session_does_nothing() {
        // A reopen racing a stop must not resurrect a session that has ended.
        let mut snapshot = BridgeSnapshot::default();
        snapshot.note_endpoint("Speakers");
        assert!(snapshot.session.is_none());
    }

    #[test]
    fn a_transient_failure_is_shown_without_calling_the_session_broken() {
        // Loss on WiFi is normal. Reporting every dropped datagram as an error
        // would train the user to ignore the status entirely.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));
        snapshot.apply(answered());
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
    fn the_label_is_the_link_that_was_established_and_not_the_address() {
        // The bug. This address is nowhere near 192.168.42/24, and the panel
        // used to call it Wi-Fi on that basis while the audio went over the
        // cable. The label now comes from the same LinkKind the packet header
        // does, so it cannot say anything the wire does not.
        let usb = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "10.114.89.244:4010".parse().unwrap(),
            LinkKind::Wired,
            false,
            true,
        );
        assert_eq!(usb.transport, "usb");
        assert_eq!(usb.transport, LinkKind::Wired.label());
    }

    #[test]
    fn an_address_in_the_android_range_reached_over_wifi_is_labelled_wifi() {
        // The mirror of the same mistake: a home network on 192.168.42/24 is
        // not a phone, and the old guess called it USB.
        let wifi = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.42.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        );
        assert_eq!(wifi.transport, "wifi");
    }

    #[test]
    fn the_multicast_group_is_labelled_as_multicast_not_guessed_at() {
        let group = SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "239.255.77.77:4010".parse().unwrap(),
            LinkKind::Multicast,
            false,
            true,
        );
        assert_eq!(group.transport, "multicast");
    }

    #[test]
    fn a_migration_moves_the_label_and_the_address_together() {
        // The panel showing the old address beside the new link, or the other
        // way round, would read as a bug in whichever half looked stale.
        let mut snapshot = BridgeSnapshot::starting(SessionInfo::new(
            "Speakers",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            false,
            true,
        ));

        snapshot.note_link("10.114.89.244:4010".parse().unwrap(), LinkKind::Wired);

        let session = snapshot.session.expect("the session survives a migration");
        assert_eq!(session.transport, "usb");
        assert_eq!(session.target, "10.114.89.244:4010");
    }

    #[test]
    fn a_migration_reported_after_the_session_stopped_changes_nothing() {
        // The watcher and the send loop race the stop button. Neither may
        // resurrect a session the user has ended.
        let mut snapshot = BridgeSnapshot::default();
        snapshot.note_link("10.114.89.244:4010".parse().unwrap(), LinkKind::Wired);
        assert!(snapshot.session.is_none());
    }

    #[test]
    fn the_session_carries_the_format_that_will_actually_be_sent() {
        let session = SessionInfo::new(
            "Headset",
            Format::stereo_48k(),
            "192.168.1.5:4010".parse().unwrap(),
            LinkKind::Wireless,
            true,
            false,
        );
        assert_eq!(session.sample_rate, 48_000);
        assert_eq!(session.channels, 2);
        assert_eq!(session.bit_depth, 16);
        assert_eq!(session.wire, "scream");
    }
}
