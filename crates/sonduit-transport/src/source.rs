//! Audio sources feeding the sender side.

use sonduit_core::format::{BitDepth, Format};

/// Something that produces interleaved PCM in a known format.
pub trait AudioSource {
    /// Format of the samples this source produces.
    fn format(&self) -> Format;

    /// Fill `out` with the next block of PCM, returning bytes written.
    ///
    /// A short fill means the source is exhausted.
    fn read(&mut self, out: &mut [u8]) -> usize;
}

/// A 440 Hz sine generator.
///
/// This is the sender end of the walking skeleton. It exists so the whole
/// chain can be exercised on a machine with no audio hardware, which is the
/// only kind of machine this project's CI has.
#[derive(Debug)]
pub struct SineSource {
    format: Format,
    frequency: f64,
    amplitude: f64,
    /// Frames produced so far. Phase is derived from this rather than
    /// accumulated, so it cannot drift over a long run.
    frames_emitted: u64,
    frames_remaining: Option<u64>,
}

impl SineSource {
    /// A source producing `frames` of 440 Hz tone, or an endless one when
    /// `frames` is `None`.
    #[must_use]
    pub const fn new(format: Format, frames: Option<u64>) -> Self {
        Self {
            format,
            frequency: 440.0,
            amplitude: 0.5,
            frames_emitted: 0,
            frames_remaining: frames,
        }
    }

    /// Override the tone frequency in hertz.
    #[must_use]
    pub const fn with_frequency(mut self, hz: f64) -> Self {
        self.frequency = hz;
        self
    }

    /// Frames produced so far.
    #[must_use]
    pub const fn frames_emitted(&self) -> u64 {
        self.frames_emitted
    }

    /// Sample value for a given frame index, in the range -1.0 to 1.0.
    #[must_use]
    pub fn sample_at(&self, frame: u64) -> f64 {
        let phase = 2.0 * std::f64::consts::PI * self.frequency * frame as f64
            / f64::from(self.format.sample_rate);
        self.amplitude * phase.sin()
    }

    fn write_sample(depth: BitDepth, value: f64, out: &mut [u8]) {
        match depth {
            BitDepth::S16 => {
                let scaled = (value * f64::from(i16::MAX)) as i16;
                out.copy_from_slice(&scaled.to_le_bytes());
            }
            BitDepth::S24 => {
                let scaled = (value * 8_388_607.0) as i32;
                out.copy_from_slice(&scaled.to_le_bytes()[..3]);
            }
            BitDepth::S32 => {
                let scaled = (value * f64::from(i32::MAX)) as i32;
                out.copy_from_slice(&scaled.to_le_bytes());
            }
        }
    }
}

impl AudioSource for SineSource {
    fn format(&self) -> Format {
        self.format
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let bytes_per_frame = self.format.bytes_per_frame();
        let sample_bytes = self.format.bit_depth.bytes();
        let mut written = 0;

        while written + bytes_per_frame <= out.len() {
            if let Some(remaining) = self.frames_remaining {
                if remaining == 0 {
                    break;
                }
                self.frames_remaining = Some(remaining - 1);
            }

            let value = self.sample_at(self.frames_emitted);
            for channel in 0..self.format.channels as usize {
                let at = written + channel * sample_bytes;
                Self::write_sample(
                    self.format.bit_depth,
                    value,
                    &mut out[at..at + sample_bytes],
                );
            }

            self.frames_emitted += 1;
            written += bytes_per_frame;
        }

        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonduit_core::format::PCM_PAYLOAD_BYTES;

    #[test]
    fn a_bounded_source_stops_after_the_requested_frames() {
        let format = Format::stereo_48k();
        let mut source = SineSource::new(format, Some(100));
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];

        let written = source.read(&mut out);
        assert_eq!(written, 100 * format.bytes_per_frame());
        assert_eq!(source.read(&mut out), 0, "exhausted source yields nothing");
    }

    #[test]
    fn an_unbounded_source_always_fills_the_buffer() {
        let mut source = SineSource::new(Format::stereo_48k(), None);
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        for _ in 0..10 {
            assert_eq!(source.read(&mut out), PCM_PAYLOAD_BYTES);
        }
        assert_eq!(source.frames_emitted(), 10 * 288);
    }

    #[test]
    fn both_channels_carry_the_same_sample() {
        let mut source = SineSource::new(Format::stereo_48k(), None);
        let mut out = vec![0_u8; 4 * 8];
        source.read(&mut out);

        for frame in out.chunks_exact(4) {
            assert_eq!(&frame[0..2], &frame[2..4], "stereo must be identical");
        }
    }

    #[test]
    fn the_waveform_crosses_zero_at_the_expected_period() {
        // 440 Hz at 48 kHz is 109.09 frames per cycle, so the sign must change
        // roughly every 54.5 frames.
        let source = SineSource::new(Format::stereo_48k(), None);
        let mut crossings = 0;
        let mut previous = source.sample_at(0);

        for frame in 1..4_800_u64 {
            let value = source.sample_at(frame);
            if previous <= 0.0 && value > 0.0 {
                crossings += 1;
            }
            previous = value;
        }

        // 4800 frames is 0.1 s, so 44 complete cycles of a 440 Hz tone.
        assert!(
            (43..=45).contains(&crossings),
            "expected about 44 cycles in 0.1 s, saw {crossings}"
        );
    }

    #[test]
    fn a_partial_frame_at_the_end_is_never_emitted() {
        let mut source = SineSource::new(Format::stereo_48k(), None);
        // Six bytes holds one 4-byte frame and two spare bytes.
        let mut out = vec![0_u8; 6];
        assert_eq!(source.read(&mut out), 4);
    }

    #[test]
    fn every_bit_depth_produces_whole_frames() {
        for depth in [BitDepth::S16, BitDepth::S24, BitDepth::S32] {
            let format = Format {
                bit_depth: depth,
                ..Format::stereo_48k()
            };
            let mut source = SineSource::new(format, None);
            let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
            let written = source.read(&mut out);
            assert_eq!(written % format.bytes_per_frame(), 0, "depth {depth:?}");
        }
    }
}
