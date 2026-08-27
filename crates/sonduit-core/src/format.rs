//! Audio stream format and its wire encoding.
//!
//! The sample-rate encoding here is the one Scream's driver uses; see
//! `docs/protocol.md` for the derivation from the driver source.

use crate::Error;

/// Bytes of PCM carried by one packet.
///
/// Chosen by Scream because it divides by 4, 6 and 8, so a whole number of
/// frames fits for every common combination of channel count and sample width.
pub const PCM_PAYLOAD_BYTES: usize = 1152;

/// Sample width in bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BitDepth {
    /// 16-bit signed little-endian.
    S16,
    /// 24-bit signed little-endian, packed in three bytes.
    S24,
    /// 32-bit signed little-endian.
    S32,
}

impl BitDepth {
    /// Bits per sample, as it appears on the wire.
    #[must_use]
    pub const fn bits(self) -> u8 {
        match self {
            Self::S16 => 16,
            Self::S24 => 24,
            Self::S32 => 32,
        }
    }

    /// Bytes per sample.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 => 4,
        }
    }

    /// Parse the wire value.
    ///
    /// # Errors
    /// Returns [`Error::BadBitDepth`] for anything other than 16, 24 or 32.
    pub const fn from_bits(bits: u8) -> Result<Self, Error> {
        match bits {
            16 => Ok(Self::S16),
            24 => Ok(Self::S24),
            32 => Ok(Self::S32),
            _ => Err(Error::BadBitDepth(bits)),
        }
    }
}

/// A stream's sample rate, channel layout and sample width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format {
    /// Frames per second.
    pub sample_rate: u32,
    /// Sample width.
    pub bit_depth: BitDepth,
    /// Channel count, 1 to 8.
    pub channels: u8,
    /// Low 16 bits of the `WAVEFORMATEXTENSIBLE` channel mask.
    pub channel_mask: u16,
}

/// Channel mask for plain stereo: front left plus front right.
pub const CHANNEL_MASK_STEREO: u16 = 0x0003;

impl Format {
    /// The baseline format: 48 kHz, 16-bit, stereo.
    #[must_use]
    pub const fn stereo_48k() -> Self {
        Self {
            sample_rate: 48_000,
            bit_depth: BitDepth::S16,
            channels: 2,
            channel_mask: CHANNEL_MASK_STEREO,
        }
    }

    /// Bytes occupied by one frame, i.e. one sample for every channel.
    #[must_use]
    pub const fn bytes_per_frame(&self) -> usize {
        self.channels as usize * self.bit_depth.bytes()
    }

    /// Frames carried by a full [`PCM_PAYLOAD_BYTES`] payload.
    ///
    /// # Errors
    /// Returns [`Error::UnrepresentableFormat`] when the payload does not hold
    /// a whole number of frames, which no receiver could split correctly.
    pub const fn frames_per_packet(&self) -> Result<usize, Error> {
        let per_frame = self.bytes_per_frame();
        if per_frame == 0 || PCM_PAYLOAD_BYTES % per_frame != 0 {
            return Err(Error::UnrepresentableFormat);
        }
        Ok(PCM_PAYLOAD_BYTES / per_frame)
    }

    /// Duration of one packet in nanoseconds.
    ///
    /// # Errors
    /// Returns [`Error::BadSampleRate`] for a zero rate, and propagates
    /// [`Format::frames_per_packet`].
    pub const fn packet_duration_nanos(&self) -> Result<u64, Error> {
        // Guarded rather than assumed: this is called from the jitter buffer,
        // which runs beside the audio path and must not be able to panic.
        if self.sample_rate == 0 {
            return Err(Error::BadSampleRate(0));
        }
        let frames = match self.frames_per_packet() {
            Ok(frames) => frames,
            Err(error) => return Err(error),
        };
        Ok((frames as u64 * 1_000_000_000) / self.sample_rate as u64)
    }

    /// Encode the sample rate as Scream's one-byte marker.
    ///
    /// Bit 7 selects the base rate, 44100 when set and 48000 when clear; the
    /// low seven bits hold the multiplier of that base.
    ///
    /// # Errors
    /// Returns [`Error::BadSampleRate`] when the rate is not an integer
    /// multiple of either base, or when the multiplier exceeds seven bits.
    pub const fn rate_marker(&self) -> Result<u8, Error> {
        let rate = self.sample_rate;
        // 44100 is tried first: a rate divisible by both bases cannot exist
        // above zero, so the order only decides the encoding of zero, which is
        // rejected anyway.
        if rate != 0 && rate % 44_100 == 0 {
            let multiplier = rate / 44_100;
            if multiplier <= 0x7F {
                return Ok(0x80 | multiplier as u8);
            }
        }
        if rate != 0 && rate % 48_000 == 0 {
            let multiplier = rate / 48_000;
            if multiplier <= 0x7F {
                return Ok(multiplier as u8);
            }
        }
        Err(Error::BadSampleRate(rate))
    }

    /// Decode a sample rate from Scream's one-byte marker.
    ///
    /// # Errors
    /// Returns [`Error::BadSampleRate`] when the multiplier is zero, which is
    /// not a rate any sender should emit.
    pub const fn rate_from_marker(marker: u8) -> Result<u32, Error> {
        let base: u32 = if marker & 0x80 != 0 { 44_100 } else { 48_000 };
        let multiplier = (marker & 0x7F) as u32;
        if multiplier == 0 {
            return Err(Error::BadSampleRate(0));
        }
        Ok(base * multiplier)
    }

    /// Validate the fields a decoder cannot take on trust.
    ///
    /// `Format` has public fields, so it can be constructed directly as well as
    /// decoded. A zero sample rate is unreachable from the wire, because
    /// [`Format::rate_from_marker`] rejects a zero multiplier, but it is
    /// reachable through the API and would divide by zero in
    /// [`Format::packet_duration_nanos`].
    ///
    /// # Errors
    /// Returns [`Error::BadSampleRate`] for a zero rate,
    /// [`Error::BadChannelCount`] outside 1 to 8, or
    /// [`Error::UnrepresentableFormat`] when the payload cannot be split into
    /// whole frames.
    pub const fn validate(&self) -> Result<(), Error> {
        if self.sample_rate == 0 {
            return Err(Error::BadSampleRate(0));
        }
        if self.channels == 0 || self.channels > 8 {
            return Err(Error::BadChannelCount(self.channels));
        }
        match self.frames_per_packet() {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_markers_match_the_table_in_the_protocol_doc() {
        let cases = [
            (44_100_u32, 0x81_u8),
            (48_000, 0x01),
            (88_200, 0x82),
            (96_000, 0x02),
            (176_400, 0x84),
            (192_000, 0x04),
        ];
        for (rate, expected) in cases {
            let format = Format {
                sample_rate: rate,
                ..Format::stereo_48k()
            };
            assert_eq!(format.rate_marker().unwrap(), expected, "rate {rate}");
            assert_eq!(Format::rate_from_marker(expected).unwrap(), rate);
        }
    }

    #[test]
    fn rate_marker_round_trips_for_every_encodable_rate() {
        for multiplier in 1..=0x7F_u32 {
            for base in [44_100_u32, 48_000] {
                let rate = base * multiplier;
                let format = Format {
                    sample_rate: rate,
                    ..Format::stereo_48k()
                };
                // 48000 * 3 == 144000 is also 44100-divisible? No: only rates
                // divisible by 44100 take the 44100 branch, so a round trip is
                // exact for every rate that encodes at all.
                let marker = format.rate_marker().unwrap();
                assert_eq!(Format::rate_from_marker(marker).unwrap(), rate);
            }
        }
    }

    #[test]
    fn zero_multiplier_is_rejected() {
        assert!(Format::rate_from_marker(0x00).is_err());
        assert!(Format::rate_from_marker(0x80).is_err());
    }

    #[test]
    fn unencodable_rates_are_rejected() {
        for rate in [0_u32, 22_050, 32_000, 47_999] {
            let format = Format {
                sample_rate: rate,
                ..Format::stereo_48k()
            };
            assert!(
                format.rate_marker().is_err(),
                "rate {rate} should not encode"
            );
        }
    }

    #[test]
    fn baseline_packet_is_288_frames_and_6ms() {
        let format = Format::stereo_48k();
        assert_eq!(format.bytes_per_frame(), 4);
        assert_eq!(format.frames_per_packet().unwrap(), 288);
        assert_eq!(format.packet_duration_nanos().unwrap(), 6_000_000);
    }

    #[test]
    fn packet_durations_match_the_protocol_doc_table() {
        let cases = [
            (48_000_u32, BitDepth::S16, 2_u8, 288_usize, 6_000_000_u64),
            (48_000, BitDepth::S24, 2, 192, 4_000_000),
            (48_000, BitDepth::S32, 2, 144, 3_000_000),
            (48_000, BitDepth::S16, 6, 96, 2_000_000),
        ];
        for (rate, depth, channels, frames, nanos) in cases {
            let format = Format {
                sample_rate: rate,
                bit_depth: depth,
                channels,
                channel_mask: 0,
            };
            assert_eq!(format.frames_per_packet().unwrap(), frames);
            assert_eq!(format.packet_duration_nanos().unwrap(), nanos);
        }
    }

    #[test]
    fn seven_channels_cannot_fill_a_payload_evenly() {
        // 7 channels * 2 bytes == 14, and 1152 % 14 != 0.
        let format = Format {
            channels: 7,
            ..Format::stereo_48k()
        };
        assert!(matches!(
            format.validate(),
            Err(Error::UnrepresentableFormat)
        ));
    }

    #[test]
    fn channel_count_bounds_are_enforced() {
        for channels in [0_u8, 9, 255] {
            let format = Format {
                channels,
                ..Format::stereo_48k()
            };
            assert!(matches!(format.validate(), Err(Error::BadChannelCount(_))));
        }
    }

    #[test]
    fn bit_depth_parsing_rejects_unknown_widths() {
        assert_eq!(BitDepth::from_bits(16).unwrap(), BitDepth::S16);
        assert_eq!(BitDepth::from_bits(24).unwrap(), BitDepth::S24);
        assert_eq!(BitDepth::from_bits(32).unwrap(), BitDepth::S32);
        for bits in [0_u8, 8, 12, 20, 64] {
            assert!(BitDepth::from_bits(bits).is_err(), "bits {bits}");
        }
    }

    #[test]
    fn a_zero_sample_rate_is_rejected_rather_than_dividing_by_zero() {
        // Format has public fields, so this is reachable through the API even
        // though rate_from_marker rejects it on the wire. It used to pass
        // validate() and then panic in packet_duration_nanos.
        let format = Format {
            sample_rate: 0,
            ..Format::stereo_48k()
        };

        assert!(matches!(format.validate(), Err(Error::BadSampleRate(0))));
        assert!(matches!(
            format.packet_duration_nanos(),
            Err(Error::BadSampleRate(0))
        ));
    }

    #[test]
    fn no_valid_format_can_panic_in_duration() {
        // Sweep the representable space; none of it may panic.
        for rate in [0_u32, 44_100, 48_000, 96_000, 192_000] {
            for channels in 0..=9_u8 {
                for depth in [BitDepth::S16, BitDepth::S24, BitDepth::S32] {
                    let format = Format {
                        sample_rate: rate,
                        bit_depth: depth,
                        channels,
                        channel_mask: 0,
                    };
                    let _ = format.validate();
                    let _ = format.frames_per_packet();
                    let _ = format.packet_duration_nanos();
                }
            }
        }
    }
}
