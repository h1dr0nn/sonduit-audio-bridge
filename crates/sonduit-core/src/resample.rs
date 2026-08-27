//! Asynchronous resampling, for drift correction.
//!
//! [`crate::ratio::RatioController`] decides how much to stretch or compress
//! the stream. This applies it.
//!
//! # Where this runs
//!
//! On the receive thread, on each packet as it arrives, before it enters the
//! jitter buffer. Not in the audio callback. The callback is realtime and must
//! not allocate or take an unbounded amount of time, and a resampler whose
//! output length varies with the ratio does both. Correcting on arrival costs
//! nothing in latency: the packet is going into a buffer either way.
//!
//! # Why polynomial rather than sinc
//!
//! The ratio here is within 500 ppm of unity. At that ratio a cubic
//! interpolator's error sits far below the noise floor of 16-bit audio, while
//! a windowed sinc costs an order of magnitude more arithmetic for a
//! correctness margin nothing can hear. It also avoids pulling an FFT
//! implementation into every Android ABI.

use rubato::{FastFixedIn, PolynomialDegree, Resampler};

use crate::format::Format;
use crate::Error;

/// How far the ratio may be moved from the one the resampler was built with.
///
/// The controller clamps itself to 500 ppm, so anything above 1.01 is
/// unreachable. Two is a bound against a construction mistake, not a limit
/// anyone will meet.
const MAX_RELATIVE_RATIO: f64 = 2.0;

/// Stretches or compresses a PCM stream by a slowly varying ratio.
///
/// Input is a fixed number of frames per call, which is what a packet carries.
/// Output length varies: that variation *is* the correction.
pub struct DriftResampler {
    inner: FastFixedIn<f32>,
    format: Format,
    /// Frames of input consumed per call.
    chunk_frames: usize,
    /// Planar float, one vector per channel. Preallocated; never resized.
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    /// Interleaved little-endian output, reused across calls.
    bytes: Vec<u8>,
    /// Ratio currently in effect.
    ratio: f64,
}

impl DriftResampler {
    /// Build a resampler for `format`, consuming `chunk_frames` per call.
    ///
    /// # Errors
    /// Propagates the format error when the format is not one this crate
    /// accepts, and returns [`Error::Resampler`] when rubato refuses the
    /// chunk size or channel count.
    pub fn new(format: Format, chunk_frames: usize) -> Result<Self, Error> {
        format.validate()?;
        if chunk_frames == 0 {
            return Err(Error::Resampler);
        }

        let channels = format.channels as usize;
        let inner = FastFixedIn::<f32>::new(
            1.0,
            MAX_RELATIVE_RATIO,
            PolynomialDegree::Cubic,
            chunk_frames,
            channels,
        )
        .map_err(|_| Error::Resampler)?;

        let max_output = inner.output_frames_max();

        Ok(Self {
            format,
            chunk_frames,
            input: vec![vec![0.0; chunk_frames]; channels],
            output: vec![vec![0.0; max_output]; channels],
            bytes: Vec::with_capacity(max_output * channels * 2),
            inner,
            ratio: 1.0,
        })
    }

    /// Frames consumed per call.
    #[must_use]
    pub const fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    /// The ratio currently applied.
    #[must_use]
    pub const fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Set the output-to-input ratio.
    ///
    /// Ignored if the resampler refuses it, because a rejected ratio means the
    /// last good one stays in effect, which is always safe.
    pub fn set_ratio(&mut self, ratio: f64) {
        if !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        // ramp = true spreads the change across the chunk rather than stepping
        // it at the boundary. A step in ratio is a discontinuity in the
        // waveform, which is a click.
        if self.inner.set_resample_ratio(ratio, true).is_ok() {
            self.ratio = ratio;
        }
    }

    /// Resample one chunk of interleaved little-endian PCM.
    ///
    /// `input` must hold exactly `chunk_frames` frames. Returns interleaved
    /// bytes in the same format, of a length that varies with the ratio.
    ///
    /// # Errors
    /// Returns [`Error::BadLength`] when the input is not exactly one chunk,
    /// and [`Error::Resampler`] when rubato rejects the call.
    pub fn process(&mut self, input: &[u8]) -> Result<&[u8], Error> {
        let channels = self.format.channels as usize;
        let expected = self.chunk_frames * channels * 2;
        if input.len() != expected {
            return Err(Error::BadLength {
                expected,
                actual: input.len(),
            });
        }

        // Deinterleave to planar float. rubato works one channel at a time,
        // and the wire format is interleaved, so this conversion is not
        // avoidable.
        for frame in 0..self.chunk_frames {
            for channel in 0..channels {
                let at = (frame * channels + channel) * 2;
                let sample = i16::from_le_bytes([input[at], input[at + 1]]);
                self.input[channel][frame] = f32::from(sample) / f32::from(i16::MAX);
            }
        }

        let (_consumed, produced) = self
            .inner
            .process_into_buffer(&self.input, &mut self.output, None)
            .map_err(|_| Error::Resampler)?;

        self.bytes.clear();
        for frame in 0..produced {
            for channel in self.output.iter().take(channels) {
                // Clamped rather than allowed to wrap: interpolation can
                // overshoot slightly past full scale between two loud samples,
                // and a wrapped sample is full-scale noise.
                let value = channel[frame].clamp(-1.0, 1.0);
                let scaled = (value * f32::from(i16::MAX)) as i16;
                self.bytes.extend_from_slice(&scaled.to_le_bytes());
            }
        }

        Ok(&self.bytes)
    }

    /// Forget the interpolation history.
    ///
    /// Called with [`crate::ratio::RatioController::reset`]: the samples either
    /// side of a stream restart are unrelated, and interpolating across the
    /// join produces a sample that belongs to neither.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.ratio = 1.0;
    }
}

impl core::fmt::Debug for DriftResampler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DriftResampler")
            .field("chunk_frames", &self.chunk_frames)
            .field("ratio", &self.ratio)
            .field("channels", &self.format.channels)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 288;

    fn resampler() -> DriftResampler {
        DriftResampler::new(Format::stereo_48k(), CHUNK).unwrap()
    }

    /// A chunk of 440 Hz tone, interleaved stereo, starting at `frame`.
    fn tone(frame: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CHUNK * 4);
        for n in 0..CHUNK {
            let phase = 2.0 * std::f64::consts::PI * 440.0 * (frame + n) as f64 / 48_000.0;
            let sample = (phase.sin() * 0.5 * f64::from(i16::MAX)) as i16;
            bytes.extend_from_slice(&sample.to_le_bytes());
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_unity_ratio_returns_the_same_number_of_frames() {
        let mut resampler = resampler();
        let out = resampler.process(&tone(0)).unwrap().len();
        // The first chunk is short by the interpolator's lookahead. The second
        // is the steady state, and that is what has to match.
        let out = resampler.process(&tone(CHUNK)).unwrap().len().max(out);
        assert!(
            (out as i64 - (CHUNK * 4) as i64).abs() <= 4 * 4,
            "produced {} bytes for {} in",
            out,
            CHUNK * 4
        );
    }

    #[test]
    fn a_ratio_above_one_produces_more_audio_than_it_consumed() {
        // This is what refills a buffer that drift has drained.
        let mut resampler = resampler();
        resampler.set_ratio(1.10);
        let _ = resampler.process(&tone(0)).unwrap();
        let produced = resampler.process(&tone(CHUNK)).unwrap().len();

        assert!(
            produced > CHUNK * 4,
            "produced {produced} bytes, no more than the {} consumed",
            CHUNK * 4
        );
    }

    #[test]
    fn a_ratio_below_one_produces_less() {
        let mut resampler = resampler();
        resampler.set_ratio(0.90);
        let _ = resampler.process(&tone(0)).unwrap();
        let produced = resampler.process(&tone(CHUNK)).unwrap().len();

        assert!(produced < CHUNK * 4, "produced {produced} bytes");
    }

    #[test]
    fn the_output_is_always_a_whole_number_of_frames() {
        // A byte count that is not a multiple of the frame size would shift
        // every subsequent sample into the wrong channel, which is a very loud
        // failure.
        let mut resampler = resampler();
        for ratio in [1.0, 1.0005, 0.9995, 1.05] {
            resampler.set_ratio(ratio);
            let produced = resampler.process(&tone(0)).unwrap().len();
            assert_eq!(produced % 4, 0, "ratio {ratio} produced {produced} bytes");
        }
    }

    #[test]
    fn a_wrong_length_chunk_is_refused_rather_than_read_past() {
        let mut resampler = resampler();
        assert!(resampler.process(&[0_u8; 16]).is_err());
        assert!(resampler.process(&[]).is_err());
    }

    #[test]
    fn a_tone_survives_a_realistic_correction_without_distorting() {
        // 200 ppm is four times any real crystal difference. If cubic
        // interpolation were going to colour the audio it would show here.
        let mut resampler = resampler();
        resampler.set_ratio(1.0002);

        let mut peak = 0_i16;
        let mut frame = 0;
        for _ in 0..20 {
            let out = resampler.process(&tone(frame)).unwrap();
            for pair in out.chunks_exact(2) {
                peak = peak.max(i16::from_le_bytes([pair[0], pair[1]]).saturating_abs());
            }
            frame += CHUNK;
        }

        // The source peaks at half scale. Interpolation may overshoot slightly
        // but must not approach clipping or collapse.
        let half = i16::MAX / 2;
        assert!(
            peak > half - 400 && peak < half + 400,
            "peak drifted to {peak}, expected about {half}"
        );
    }

    #[test]
    fn a_non_finite_ratio_is_ignored_rather_than_applied() {
        let mut resampler = resampler();
        resampler.set_ratio(1.0002);
        resampler.set_ratio(f64::NAN);
        resampler.set_ratio(0.0);
        resampler.set_ratio(-1.0);

        assert_eq!(resampler.ratio(), 1.0002);
    }

    #[test]
    fn a_reset_returns_to_unity() {
        let mut resampler = resampler();
        resampler.set_ratio(1.05);
        resampler.reset();
        assert_eq!(resampler.ratio(), 1.0);
    }

    #[test]
    fn a_mono_stream_is_handled_as_well_as_a_stereo_one() {
        let format = Format {
            channels: 1,
            channel_mask: 0x0004,
            ..Format::stereo_48k()
        };
        let mut resampler = DriftResampler::new(format, CHUNK).unwrap();
        let input = vec![0_u8; CHUNK * 2];

        let produced = resampler.process(&input).unwrap().len();
        assert_eq!(produced % 2, 0);
    }

    #[test]
    fn a_zero_length_chunk_is_refused_at_construction() {
        assert!(DriftResampler::new(Format::stereo_48k(), 0).is_err());
    }
}
