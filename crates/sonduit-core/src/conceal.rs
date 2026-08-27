//! Packet loss concealment: what to play when a packet never arrived.
//!
//! # Why not silence
//!
//! Silence is the cheapest thing to emit and the worst thing to hear. A gap of
//! silence puts a step discontinuity at *both* of its edges, and a step is
//! broadband: a single 6 ms hole in a 440 Hz tone is not heard as a 6 ms hole,
//! it is heard as two clicks. The clicks are louder and far more noticeable
//! than the missing audio itself, which is why every telephony codec conceals
//! rather than mutes.
//!
//! # Why a repeated pitch period
//!
//! Audio that is worth listening to is locally periodic: speech has a glottal
//! period, music has a fundamental, and both change slowly compared with the
//! six milliseconds a packet covers. So the best available guess at the audio
//! that was lost is the audio just before it, replayed at its own period.
//! Copying exactly one period back means the first concealed sample continues
//! the waveform where the last real sample left off, in amplitude and in
//! phase, so the leading edge of the gap has no step at all. Only the trailing
//! edge keeps one, and by then the level has been reduced, so one soft
//! discontinuity replaces two hard ones.
//!
//! This is the technique in ITU-T G.711 Appendix I, and it is chosen here for
//! reasons rather than for its pedigree:
//!
//! - It costs one correlation per *run* of losses, not per packet, and the
//!   synthesis itself is a load, a multiply-add and a store per sample. The
//!   search is bounded by [`MIN_PITCH_HZ`] and [`MAX_PITCH_HZ`] and by an
//!   analysis window of a few milliseconds; at 48 kHz that is roughly 1.4e5
//!   multiply-accumulates once per loss run, which fits inside a packet period
//!   with room to spare even on a phone.
//! - It needs no state that cannot be preallocated. Everything here is sized
//!   from the [`Format`] at construction and never grows, so the audio path
//!   neither allocates nor frees.
//! - It degrades honestly. Extending a period for 6 ms is inaudible; extending
//!   it for 200 ms is a robotic buzz that is worse than the hole it fills, so
//!   the output is faded to true zero between [`FADE_START_MS`] and
//!   [`FADE_END_MS`], and the segment being repeated is widened as the run
//!   grows so that a long erasure is not one pitch pulse on a loop.
//!
//! # What was rejected
//!
//! - *Blind waveform substitution*, replaying the last packet as it stands. It
//!   is cheaper still, but the copy starts at an arbitrary phase, so it
//!   reintroduces the very step at the leading edge that concealment exists to
//!   remove.
//! - *Noise substitution*, filling the gap with shaped noise. That is what a
//!   comfort noise generator does and it is right for a talker pausing, but
//!   this stream carries music and game audio at least as often as speech, and
//!   noise in place of a tone is heard as a dropout with a hiss on top.
//! - *Model based extrapolation*, LPC or anything above it. It does sound
//!   better across long erasures, and it wants a solver on the audio path for
//!   the case that the fade above has already given up on.
//! - *Overlap-adding into the first packet that does arrive*, which G.711
//!   Appendix I also does at the trailing edge. That would mean this module
//!   editing audio that was received intact, on the strength of a pitch
//!   estimate that may be wrong; a bad estimate would then damage good audio
//!   instead of merely filling a hole imperfectly. The trailing step is left
//!   in place instead, at reduced level.
//!
//! Nothing here reads a clock or allocates after construction, in keeping with
//! the crate rule in CONTRIBUTING.md.

use crate::format::{BitDepth, Format};

/// Lowest fundamental the period search considers.
///
/// Below this, one period is longer than the history worth keeping, and a male
/// speaking voice or a bass note is already inside the range.
pub const MIN_PITCH_HZ: u32 = 60;

/// Highest fundamental the period search considers.
///
/// Above this the period is short enough that any multiple of it is also a
/// period, so the search settles on one of those and the result is the same.
pub const MAX_PITCH_HZ: u32 = 500;

/// Length of a loss run that is concealed at full level.
///
/// One or two lost packets are the overwhelmingly common case and must not be
/// attenuated at all; a listener should not be able to tell anything happened.
pub const FADE_START_MS: u32 = 10;

/// Length of a loss run after which the output is exactly zero.
///
/// Past this the periodic extension has stopped resembling the sender, and the
/// honest answer is that the audio is gone.
pub const FADE_END_MS: u32 = 60;

/// Pitch periods spliced together at the longest erasure.
///
/// Repeating a single period for tens of milliseconds is heard as a buzz
/// because it is perfectly regular, which natural audio never is. Widening the
/// repeated segment one period at a time keeps more of the original variation.
const MAX_REPEAT_PERIODS: usize = 3;

/// Milliseconds of erasure between each widening of the repeated segment.
const REPEAT_STEP_MS: u32 = 10;

/// Length of the correlation window, counted in shortest periods considered.
///
/// Two periods is long enough to tell a period from half of one, and short
/// enough that the search stays cheap.
const ANALYSIS_PERIODS: usize = 2;

/// Fraction of a period cross-faded where the repeated segment wraps.
///
/// The wrap is only approximately continuous, because the estimated period is
/// a whole number of frames and the true one is not. A quarter period of
/// cross-fade hides the residue, which is the same choice G.711 Appendix I
/// makes and for the same reason.
const OVERLAP_DIVISOR: usize = 4;

/// Reads one sample of `depth` from the front of `bytes`.
fn decode_sample(bytes: &[u8], depth: BitDepth) -> f32 {
    match depth {
        BitDepth::S16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        // Sign extension by putting the three bytes at the top of an i32 and
        // shifting back down. Masking instead would read every negative sample
        // as a large positive one.
        BitDepth::S24 => (i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32,
        BitDepth::S32 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32,
    }
}

/// Writes one sample of `depth` to the front of `bytes`, saturating.
fn encode_sample(value: f32, depth: BitDepth, bytes: &mut [u8]) {
    // A float to integer cast in Rust saturates rather than wrapping, so a
    // value outside the range clips instead of inverting its phase.
    let sample = value.round() as i32;
    match depth {
        BitDepth::S16 => {
            let sample = sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            bytes[..2].copy_from_slice(&sample.to_le_bytes());
        }
        BitDepth::S24 => {
            let sample = sample.clamp(-8_388_608, 8_388_607);
            bytes[..3].copy_from_slice(&sample.to_le_bytes()[..3]);
        }
        BitDepth::S32 => {
            bytes[..4].copy_from_slice(&sample.to_le_bytes());
        }
    }
}

/// Synthesises a packet of audio to play in place of one that was lost.
///
/// Feed every packet that does arrive to [`PacketConcealer::observe`], and ask
/// [`PacketConcealer::conceal`] for a replacement whenever the jitter buffer
/// reports [`crate::jitter::PopOutcome::Lost`]. The replacement is always
/// exactly one packet long, so the playback timeline stays aligned with the
/// timeline the sender is producing.
#[derive(Debug)]
pub struct PacketConcealer {
    format: Format,
    channels: usize,
    frames_per_packet: usize,
    /// Frames of history held. Zero for a format whose payload does not divide
    /// into whole frames, which disables concealment rather than guessing.
    capacity: usize,
    min_period: usize,
    max_period: usize,
    /// Interleaved history of what was played, in raw sample units.
    ///
    /// A ring, so a packet that arrives costs one store per sample and no
    /// shuffling of what is already held.
    history: Vec<f32>,
    /// Frame average of `history`, the signal the period search runs on.
    ///
    /// The search runs on the downmix rather than on channel zero so that a
    /// hard panned source still yields the period the other channels share.
    /// Every channel is then continued at the same lag, which is what keeps
    /// their phase relationship intact through the gap.
    mono_history: Vec<f32>,
    write: usize,
    filled: usize,
    /// `history` unwrapped oldest first, rebuilt once per loss run.
    ///
    /// Nothing is received during a run, so the unwrapped copy stays valid for
    /// the whole of it and both the search and the synthesis can index it
    /// directly instead of paying for the wrap on every sample.
    linear: Vec<f32>,
    linear_mono: Vec<f32>,
    available: usize,
    /// Estimated period in frames, or zero when there is nothing to repeat.
    period: usize,
    /// Next frame of `linear` the synthesis will read.
    read: usize,
    /// Frames emitted since the last packet that actually arrived.
    run_frames: usize,
    fade_start_frames: usize,
    fade_span_frames: usize,
    repeat_step_frames: usize,
    /// One packet of concealment. Never resized, so `conceal` cannot allocate.
    output: Vec<u8>,
}

impl PacketConcealer {
    /// Build a concealer for `format`, allocating everything it will ever use.
    ///
    /// A format whose payload does not divide into whole frames produces a
    /// concealer that emits nothing, which is what the jitter buffer already
    /// does with such a format: there is no frame count to conceal, and
    /// inventing one would hand a fractional frame to the device.
    #[must_use]
    pub fn new(format: Format) -> Self {
        let channels = format.channels as usize;
        let frames_per_packet = format.frames_per_packet().unwrap_or(0);
        let rate = format.sample_rate;

        let min_period = (rate / MAX_PITCH_HZ).max(1) as usize;
        let max_period = (rate / MIN_PITCH_HZ).max(1) as usize;
        let window = min_period * ANALYSIS_PERIODS;

        // Enough history for the longest segment the synthesis can repeat plus
        // the window the search compares against it. Sized once, here, so that
        // neither of them can ever ask for more.
        let capacity = if frames_per_packet == 0 {
            0
        } else {
            max_period * MAX_REPEAT_PERIODS + window
        };

        let per_ms = f64::from(rate) / 1000.0;
        let fade_start_frames = (per_ms * f64::from(FADE_START_MS)) as usize;
        let fade_end_frames = (per_ms * f64::from(FADE_END_MS)) as usize;

        Self {
            format,
            channels,
            frames_per_packet,
            capacity,
            min_period,
            max_period,
            history: vec![0.0; capacity * channels],
            mono_history: vec![0.0; capacity],
            write: 0,
            filled: 0,
            linear: vec![0.0; capacity * channels],
            linear_mono: vec![0.0; capacity],
            available: 0,
            period: 0,
            read: 0,
            run_frames: 0,
            fade_start_frames,
            fade_span_frames: fade_end_frames.saturating_sub(fade_start_frames),
            repeat_step_frames: (per_ms * f64::from(REPEAT_STEP_MS)) as usize,
            output: vec![0; frames_per_packet * format.bytes_per_frame()],
        }
    }

    /// Bytes in one concealment packet.
    #[must_use]
    pub fn packet_bytes(&self) -> usize {
        self.output.len()
    }

    /// Frames in one concealment packet.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames_per_packet
    }

    /// Take note of a packet that did arrive.
    ///
    /// Only whole frames are read, so a short or ragged payload is truncated
    /// rather than rejected: concealment quality is worth degrading, the audio
    /// path is not worth panicking on.
    pub fn observe(&mut self, pcm: &[u8]) {
        // A received packet ends any loss run: the next one starts from real
        // audio again, at full level and with a fresh period estimate.
        self.run_frames = 0;

        if self.capacity == 0 || self.channels == 0 {
            return;
        }

        let width = self.format.bit_depth.bytes();
        let bytes_per_frame = self.format.bytes_per_frame();
        let frames = pcm.len() / bytes_per_frame;

        for frame in 0..frames {
            let base = frame * bytes_per_frame;
            let slot = self.write;
            let mut sum = 0.0;
            for channel in 0..self.channels {
                let at = base + channel * width;
                let value = decode_sample(&pcm[at..at + width], self.format.bit_depth);
                self.history[slot * self.channels + channel] = value;
                sum += value;
            }
            self.mono_history[slot] = sum / self.channels as f32;
            self.write = (slot + 1) % self.capacity;
            self.filled = (self.filled + 1).min(self.capacity);
        }
    }

    /// The packet the most recent [`PacketConcealer::conceal`] produced.
    ///
    /// Separate from `conceal` because a device callback drains a packet over
    /// several bursts and has to be able to read the rest of one without
    /// synthesising another.
    #[must_use]
    pub fn packet(&self) -> &[u8] {
        &self.output
    }

    /// Synthesise one packet of replacement audio.
    ///
    /// The result is always exactly [`PacketConcealer::packet_bytes`] long,
    /// including before any audio has been observed, when it is silence
    /// because there is nothing yet to continue.
    pub fn conceal(&mut self) -> &[u8] {
        if self.output.is_empty() {
            return &self.output;
        }

        if self.run_frames == 0 {
            self.prepare();
        }

        if self.period == 0 {
            self.output.fill(0);
            self.run_frames += self.frames_per_packet;
            return &self.output;
        }

        let depth = self.format.bit_depth;
        let width = depth.bytes();
        let channels = self.channels;
        let overlap = (self.period / OVERLAP_DIVISOR).max(1);
        let blend_from = self.available.saturating_sub(overlap);

        for frame in 0..self.frames_per_packet {
            let elapsed = self.run_frames + frame;
            let gain = self.gain_at(elapsed);
            let cycle = self.cycle_at(elapsed);
            let read = self.read;

            // Inside the last frames of the history the output is faded
            // towards the frames one whole segment earlier, which is where the
            // read cursor is about to jump. Without it, the jump is a step
            // every time the segment repeats.
            let weight = if read >= blend_from && read >= cycle {
                (read - blend_from + 1) as f32 / (overlap + 1) as f32
            } else {
                0.0
            };

            for channel in 0..channels {
                let mut value = self.linear[read * channels + channel];
                if weight > 0.0 {
                    let wrapped = self.linear[(read - cycle) * channels + channel];
                    value = value * (1.0 - weight) + wrapped * weight;
                }
                let at = (frame * channels + channel) * width;
                encode_sample(value * gain, depth, &mut self.output[at..at + width]);
            }

            self.read += 1;
            if self.read >= self.available {
                self.read = self.available - cycle;
            }
        }

        self.run_frames += self.frames_per_packet;
        &self.output
    }

    /// Unwrap the history and estimate the period, once per loss run.
    fn prepare(&mut self) {
        self.available = self.filled;
        self.period = 0;
        self.read = 0;

        if self.available == 0 {
            return;
        }

        let start = (self.write + self.capacity - self.available) % self.capacity;
        for frame in 0..self.available {
            let source = (start + frame) % self.capacity;
            self.linear_mono[frame] = self.mono_history[source];
            for channel in 0..self.channels {
                self.linear[frame * self.channels + channel] =
                    self.history[source * self.channels + channel];
            }
        }

        self.period = self.estimate_period();
        if self.period > 0 {
            self.read = self.available - self.cycle_at(0);
        }
    }

    /// Lag in frames at which the recent past best predicts itself.
    ///
    /// Normalised by the energy of the candidate segment rather than taken
    /// raw, because an unnormalised correlation prefers whichever lag points
    /// at the loudest audio instead of the one that matches shape.
    ///
    /// Returns zero when no lag correlates positively, which covers silence
    /// and a history too short to hold even the shortest period the search
    /// allows. The caller then emits silence, because there is genuinely
    /// nothing to continue.
    fn estimate_period(&self) -> usize {
        let window = (self.min_period * ANALYSIS_PERIODS)
            .min(self.available / 2)
            .max(1);
        let max_lag = self.max_period.min(self.available.saturating_sub(window));
        if max_lag < self.min_period {
            return 0;
        }

        let tail = self.available - window;
        let mut best_lag = 0;
        let mut best_score = 0.0;

        for lag in self.min_period..=max_lag {
            let mut correlation = 0.0;
            let mut energy = 0.0;
            for index in 0..window {
                let recent = self.linear_mono[tail + index];
                let past = self.linear_mono[tail + index - lag];
                correlation += recent * past;
                energy += past * past;
            }

            if correlation <= 0.0 || energy <= 0.0 {
                continue;
            }
            let score = correlation / energy.sqrt();
            if score > best_score {
                best_score = score;
                best_lag = lag;
            }
        }

        best_lag
    }

    /// Length in frames of the segment being repeated, `elapsed` frames in.
    fn cycle_at(&self, elapsed: usize) -> usize {
        // checked_div rather than a guard: a sample rate small enough to make
        // the step zero is unreachable from the wire but reachable through the
        // public Format fields, and this runs on the audio path.
        let wanted = elapsed
            .checked_div(self.repeat_step_frames)
            .map_or(MAX_REPEAT_PERIODS, |steps| {
                (steps + 1).min(MAX_REPEAT_PERIODS)
            });
        // Never more periods than the history actually holds, and never a
        // segment that is not a whole number of periods: a truncated segment
        // would put a step at every wrap, which is exactly what the pitch
        // alignment exists to avoid.
        let usable = (self.available / self.period.max(1)).max(1);
        (self.period * wanted.min(usable)).max(1)
    }

    /// Output level `elapsed` frames into a loss run.
    fn gain_at(&self, elapsed: usize) -> f32 {
        if elapsed < self.fade_start_frames {
            return 1.0;
        }
        if self.fade_span_frames == 0 {
            return 0.0;
        }
        let into = elapsed - self.fade_start_frames;
        if into >= self.fade_span_frames {
            return 0.0;
        }
        1.0 - into as f32 / self.fade_span_frames as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Largest positive value `depth` can carry.
    fn full_scale(depth: BitDepth) -> f64 {
        match depth {
            BitDepth::S16 => f64::from(i16::MAX),
            BitDepth::S24 => 8_388_607.0,
            BitDepth::S32 => f64::from(i32::MAX),
        }
    }

    /// `frames` of interleaved tone, one frequency per channel, starting at
    /// absolute frame `from` so successive calls stay phase continuous.
    fn tone(format: Format, hz: &[f64], from: usize, frames: usize) -> Vec<u8> {
        let width = format.bit_depth.bytes();
        let channels = format.channels as usize;
        let amplitude = full_scale(format.bit_depth) * 0.4;
        let mut pcm = vec![0_u8; frames * format.bytes_per_frame()];

        for frame in 0..frames {
            for channel in 0..channels {
                let frequency = hz[channel % hz.len()];
                let phase =
                    2.0 * PI * frequency * (from + frame) as f64 / f64::from(format.sample_rate);
                let value = phase.sin() * amplitude;
                let at = (frame * channels + channel) * width;
                encode_sample(value as f32, format.bit_depth, &mut pcm[at..at + width]);
            }
        }
        pcm
    }

    /// Every sample of `pcm`, interleaved, in raw sample units.
    fn samples(pcm: &[u8], format: Format) -> Vec<f64> {
        let width = format.bit_depth.bytes();
        (0..pcm.len() / width)
            .map(|index| {
                f64::from(decode_sample(
                    &pcm[index * width..(index + 1) * width],
                    format.bit_depth,
                ))
            })
            .collect()
    }

    /// Mean squared error between two packets, over one channel or all of them.
    fn mse(candidate: &[u8], truth: &[u8], format: Format, channel: Option<usize>) -> f64 {
        let channels = format.channels as usize;
        let candidate = samples(candidate, format);
        let truth = samples(truth, format);
        assert_eq!(candidate.len(), truth.len());

        let (mut total, mut count) = (0.0, 0);
        for (index, (a, b)) in candidate.iter().zip(truth.iter()).enumerate() {
            if channel.is_some_and(|wanted| index % channels != wanted) {
                continue;
            }
            total += (a - b) * (a - b);
            count += 1;
        }
        total / count as f64
    }

    fn rms(pcm: &[u8], format: Format) -> f64 {
        let values = samples(pcm, format);
        (values.iter().map(|value| value * value).sum::<f64>() / values.len() as f64).sqrt()
    }

    /// Feed `packets` packets of tone, then return the concealment for the
    /// packet after them alongside the audio that packet really carried.
    fn conceal_after_tone(
        format: Format,
        hz: &[f64],
        packets: usize,
    ) -> (PacketConcealer, Vec<u8>, Vec<u8>) {
        let frames = format.frames_per_packet().unwrap();
        let mut concealer = PacketConcealer::new(format);

        for packet in 0..packets {
            let pcm = tone(format, hz, packet * frames, frames);
            concealer.observe(&pcm);
        }

        let truth = tone(format, hz, packets * frames, frames);
        let concealed = concealer.conceal().to_vec();
        (concealer, concealed, truth)
    }

    #[test]
    fn one_lost_packet_is_closer_to_the_truth_than_silence_is() {
        // The reason this module exists, stated as a measurement rather than
        // as a description of the algorithm: if concealment is not measurably
        // nearer the audio that was lost than silence is, it is not worth the
        // cycles it costs.
        let format = Format::stereo_48k();
        let (_, concealed, truth) = conceal_after_tone(format, &[440.0], 6);

        let silence = vec![0_u8; concealed.len()];
        let concealed_mse = mse(&concealed, &truth, format, None);
        let silence_mse = mse(&silence, &truth, format, None);

        println!(
            "440 Hz stereo: conceal mse {concealed_mse:.1}, silence mse {silence_mse:.1}, \
             ratio {:.1}x",
            silence_mse / concealed_mse
        );
        assert!(
            concealed_mse * 10.0 < silence_mse,
            "concealment mse {concealed_mse} is not far below silence mse {silence_mse}"
        );
    }

    #[test]
    fn the_same_holds_for_a_mono_stream() {
        let format = Format {
            channels: 1,
            channel_mask: 0x0004,
            ..Format::stereo_48k()
        };
        let (_, concealed, truth) = conceal_after_tone(format, &[440.0], 6);

        let silence = vec![0_u8; concealed.len()];
        let concealed_mse = mse(&concealed, &truth, format, None);
        let silence_mse = mse(&silence, &truth, format, None);

        println!(
            "440 Hz mono: conceal mse {concealed_mse:.1}, silence mse {silence_mse:.1}, \
             ratio {:.1}x",
            silence_mse / concealed_mse
        );
        assert!(concealed_mse * 10.0 < silence_mse);
    }

    #[test]
    fn every_channel_is_continued_on_its_own_content() {
        // 220 and 660 Hz share a period, so the downmix the search runs on has
        // one, but the two channels only stay right if the lag is applied to
        // each channel separately. Continuing both from channel zero would
        // pass a single channel test and fail this one.
        let format = Format::stereo_48k();
        let (_, concealed, truth) = conceal_after_tone(format, &[220.0, 660.0], 8);
        let silence = vec![0_u8; concealed.len()];

        for channel in 0..2 {
            let concealed_mse = mse(&concealed, &truth, format, Some(channel));
            let silence_mse = mse(&silence, &truth, format, Some(channel));
            println!(
                "channel {channel}: conceal mse {concealed_mse:.1}, silence mse {silence_mse:.1}"
            );
            assert!(
                concealed_mse * 10.0 < silence_mse,
                "channel {channel} concealed at mse {concealed_mse} against silence {silence_mse}"
            );
        }
    }

    #[test]
    fn a_silent_channel_is_not_filled_with_the_other_one() {
        // Catches an interleaving mistake that averages or cross-copies
        // channels, which a test on a correlated stereo pair cannot see.
        let format = Format::stereo_48k();
        let frames = format.frames_per_packet().unwrap();
        let mut concealer = PacketConcealer::new(format);

        for packet in 0..8 {
            let mut pcm = tone(format, &[300.0], packet * frames, frames);
            for frame in 0..frames {
                // Silence the right channel of every frame.
                pcm[frame * 4 + 2] = 0;
                pcm[frame * 4 + 3] = 0;
            }
            concealer.observe(&pcm);
        }

        let concealed = concealer.conceal().to_vec();
        let values = samples(&concealed, format);
        assert!(
            values.iter().step_by(2).any(|value| value.abs() > 1.0),
            "the channel that carried audio was concealed with silence"
        );
        assert!(
            values.iter().skip(1).step_by(2).all(|value| *value == 0.0),
            "a silent channel picked up audio from its neighbour"
        );
    }

    #[test]
    fn a_run_of_losses_fades_to_silence_rather_than_looping_forever() {
        let format = Format::stereo_48k();
        let frames = format.frames_per_packet().unwrap();
        let mut concealer = PacketConcealer::new(format);
        for packet in 0..8 {
            let pcm = tone(format, &[440.0], packet * frames, frames);
            concealer.observe(&pcm);
        }

        // 20 packets is 120 ms, twice the length of the fade.
        let levels: Vec<f64> = (0..20)
            .map(|_| {
                let packet = concealer.conceal().to_vec();
                rms(&packet, format)
            })
            .collect();

        assert!(
            levels[0] > full_scale(BitDepth::S16) * 0.2,
            "the first concealed packet is already quiet: {}",
            levels[0]
        );
        for window in levels.windows(2) {
            assert!(
                window[1] <= window[0] * 1.05 + 1.0,
                "level rose from {} to {} during a loss run",
                window[0],
                window[1]
            );
        }
        assert!(levels[6] < levels[2], "the fade never started");

        // FADE_END_MS is 60, so every packet that begins at or after 60 ms
        // must be exactly zero and not merely small.
        let first_silent = (f64::from(FADE_END_MS) * 48.0 / frames as f64).ceil() as usize;
        for (index, level) in levels.iter().enumerate().skip(first_silent) {
            assert_eq!(*level, 0.0, "packet {index} of the run is still sounding");
        }
    }

    #[test]
    fn a_packet_that_arrives_restores_full_level() {
        // The fade is per run, not cumulative. A stream that loses a packet
        // every few seconds must not get quieter over an evening.
        let format = Format::stereo_48k();
        let frames = format.frames_per_packet().unwrap();
        let mut concealer = PacketConcealer::new(format);
        for packet in 0..8 {
            let pcm = tone(format, &[440.0], packet * frames, frames);
            concealer.observe(&pcm);
        }

        for _ in 0..20 {
            concealer.conceal();
        }
        assert_eq!(rms(concealer.packet(), format), 0.0);

        concealer.observe(&tone(format, &[440.0], 0, frames));
        let recovered = concealer.conceal().to_vec();
        assert!(
            rms(&recovered, format) > full_scale(BitDepth::S16) * 0.2,
            "concealment stayed faded after audio resumed"
        );
    }

    #[test]
    fn concealment_is_exactly_one_packet_for_every_supported_format() {
        // The timeline only stays aligned with the sender if this is true for
        // every format the wire allows, not only for the baseline one.
        for rate in [44_100_u32, 48_000, 96_000] {
            for channels in 1..=8_u8 {
                for depth in [BitDepth::S16, BitDepth::S24, BitDepth::S32] {
                    let format = Format {
                        sample_rate: rate,
                        bit_depth: depth,
                        channels,
                        channel_mask: 0,
                    };
                    if format.validate().is_err() {
                        continue;
                    }

                    let frames = format.frames_per_packet().unwrap();
                    let expected = frames * format.bytes_per_frame();
                    let mut concealer = PacketConcealer::new(format);
                    assert_eq!(concealer.packet_bytes(), expected);
                    assert_eq!(concealer.frames(), frames);

                    // Before any audio, after a little, and deep into a run.
                    assert_eq!(concealer.conceal().len(), expected);
                    for packet in 0..40 {
                        concealer.observe(&tone(format, &[220.0], packet * frames, frames));
                    }
                    for _ in 0..30 {
                        assert_eq!(
                            concealer.conceal().len(),
                            expected,
                            "{rate} Hz, {channels} channels, {} bits",
                            depth.bits()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn concealing_before_any_audio_has_been_seen_is_silence() {
        // Reachable whenever the first packets of a stream are the ones that
        // go missing. There is nothing to continue, and inventing something
        // would be worse than the gap.
        let format = Format::stereo_48k();
        let mut concealer = PacketConcealer::new(format);
        let expected = concealer.packet_bytes();
        for _ in 0..5 {
            let packet = concealer.conceal();
            assert_eq!(packet.len(), expected);
            assert!(packet.iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn a_history_shorter_than_a_period_conceals_with_silence() {
        // Half a millisecond of audio cannot support a period estimate, and
        // repeating it would be a tone this stream never carried.
        let format = Format::stereo_48k();
        let mut concealer = PacketConcealer::new(format);
        concealer.observe(&tone(format, &[440.0], 0, 20));
        assert!(concealer.conceal().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_format_that_cannot_be_split_conceals_nothing_instead_of_panicking() {
        // 7 channels at 16 bits does not divide 1152, so no receiver could
        // split such a packet in the first place.
        let format = Format {
            channels: 7,
            ..Format::stereo_48k()
        };
        let mut concealer = PacketConcealer::new(format);
        concealer.observe(&[1_u8; 1152]);
        assert_eq!(concealer.packet_bytes(), 0);
        assert!(concealer.conceal().is_empty());
    }

    #[test]
    fn a_silent_stream_is_concealed_with_silence() {
        // An all-zero history has no period, and the normalised correlation
        // divides by its energy, so this is also the divide-by-zero case.
        let format = Format::stereo_48k();
        let frames = format.frames_per_packet().unwrap();
        let mut concealer = PacketConcealer::new(format);
        for _ in 0..8 {
            concealer.observe(&vec![0_u8; frames * format.bytes_per_frame()]);
        }
        assert!(concealer.conceal().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn the_estimated_period_is_the_one_the_tone_actually_has() {
        // The concealment can be close to the truth for a packet or two even
        // with a wrong period, so the estimate is pinned directly: 48000/440
        // is 109.09 frames, and the search may legitimately settle on a
        // multiple of it.
        let format = Format::stereo_48k();
        let (concealer, _, _) = conceal_after_tone(format, &[440.0], 8);
        let period = concealer.period as f64;
        let true_period = 48_000.0 / 440.0;
        let ratio = period / true_period;
        assert!(
            (ratio - ratio.round()).abs() < 0.02 && ratio.round() >= 1.0,
            "estimated period {period} is not a multiple of {true_period}"
        );
    }
}
