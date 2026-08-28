//! Packet encoding for both wire formats Sonduit speaks.
//!
//! Two formats exist because Scream's five-byte header has no spare bits and
//! carries neither a sequence number nor a timestamp. See ADR-005.
//!
//! * [`ScreamPacket`] is the compatibility format, byte-for-byte what the
//!   Scream driver emits. Sonduit can receive it so an unmodified driver works
//!   as a sender out of the box.
//! * [`SonduitPacket`] is the native format. It adds a magic value, a version,
//!   a sequence number and a sender timestamp, which is what makes loss
//!   detection, reordering repair, jitter estimation and drift correction
//!   possible at all.

use crate::format::{BitDepth, Format};

pub use crate::format::PCM_PAYLOAD_BYTES;
use crate::Error;

/// Size of Scream's header.
pub const SCREAM_HEADER_BYTES: usize = 5;

/// Total size of a Scream datagram. The driver only ever sends this exact size.
pub const SCREAM_PACKET_BYTES: usize = SCREAM_HEADER_BYTES + PCM_PAYLOAD_BYTES;

/// Size of Sonduit's header.
pub const SONDUIT_HEADER_BYTES: usize = 20;

/// Magic prefix identifying a Sonduit datagram.
///
/// Scream has no magic, so a receiver bound to the port would otherwise decode
/// any datagram of the right length as audio.
pub const SONDUIT_MAGIC: [u8; 4] = *b"SDT1";

/// Wire format version carried in the header.
pub const SONDUIT_VERSION: u8 = 1;

/// Version reserved for the encrypted format, which lives in
/// `sonduit-transport`.
///
/// It is named here so that the two crates cannot drift into using the same
/// byte for different things, and so that [`SonduitPacket::decode`]'s refusal
/// of it is a documented decision rather than an accident of the version
/// check.
///
/// Encryption is a version bump and not a flag bit. A flag would have been
/// ignored by every receiver already built, which would decode the ciphertext
/// as PCM and play it: a full-scale noise burst into somebody's headphones. A
/// version this build does not know is refused here, before a single byte of
/// payload is looked at. See ADR-009.
///
/// The key that opens such a packet is agreed during pairing, so the two ends
/// have already settled whether they can encrypt before any audio flows; a
/// receiver never has to guess.
pub const SONDUIT_VERSION_SEALED: u8 = 2;

/// A decoded packet: the format it declares, plus a borrowed PCM payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreamPacket<'a> {
    /// Format declared by the header.
    pub format: Format,
    /// Raw interleaved little-endian PCM.
    pub pcm: &'a [u8],
}

impl<'a> ScreamPacket<'a> {
    /// Decode a Scream datagram.
    ///
    /// # Errors
    /// Returns [`Error::BadLength`] unless the datagram is exactly
    /// [`SCREAM_PACKET_BYTES`], and propagates any format validation failure.
    pub fn decode(datagram: &'a [u8]) -> Result<Self, Error> {
        if datagram.len() != SCREAM_PACKET_BYTES {
            return Err(Error::BadLength {
                expected: SCREAM_PACKET_BYTES,
                actual: datagram.len(),
            });
        }

        let format = Format {
            sample_rate: Format::rate_from_marker(datagram[0])?,
            bit_depth: BitDepth::from_bits(datagram[1])?,
            channels: datagram[2],
            channel_mask: u16::from_le_bytes([datagram[3], datagram[4]]),
        };
        format.validate()?;

        Ok(Self {
            format,
            pcm: &datagram[SCREAM_HEADER_BYTES..],
        })
    }

    /// Encode into `out`, which must be exactly [`SCREAM_PACKET_BYTES`] long.
    ///
    /// # Errors
    /// Returns [`Error::BadLength`] when `out` is the wrong size or the
    /// payload is not [`PCM_PAYLOAD_BYTES`], and propagates format failures.
    pub fn encode(format: &Format, pcm: &[u8], out: &mut [u8]) -> Result<(), Error> {
        if pcm.len() != PCM_PAYLOAD_BYTES {
            return Err(Error::BadLength {
                expected: PCM_PAYLOAD_BYTES,
                actual: pcm.len(),
            });
        }
        if out.len() != SCREAM_PACKET_BYTES {
            return Err(Error::BadLength {
                expected: SCREAM_PACKET_BYTES,
                actual: out.len(),
            });
        }
        format.validate()?;

        let mask = format.channel_mask.to_le_bytes();
        out[0] = format.rate_marker()?;
        out[1] = format.bit_depth.bits();
        out[2] = format.channels;
        out[3] = mask[0];
        out[4] = mask[1];
        out[SCREAM_HEADER_BYTES..].copy_from_slice(pcm);
        Ok(())
    }
}

/// Flag bit 0: the sender is on a wired link.
///
/// The receiver sizes its jitter buffer from this. It cannot work the answer
/// out for itself: it only sees the address the datagrams came from, and USB
/// tethering hands out whatever range the phone's driver chose -- a real
/// device here reported 10.114.89.x, nothing like the 192.168.42/24 that
/// guess assumed, so a wired link was treated as Wi-Fi and held 30 ms instead
/// of 10. The sender does know, because it picked the interface.
///
/// A sender that does not set it is not claiming Wi-Fi, only declining to say,
/// which is also what every pre-flag sender does. The receiver falls back to
/// guessing in that case, so this is additive on the wire.
pub const FLAG_WIRED_LINK: u8 = 0b0000_0001;

/// Sonduit's native packet.
///
/// Layout, all integers little-endian:
///
/// ```text
///  0..4   magic "SDT1"
///  4      version
///  5      flags, see [`FLAG_WIRED_LINK`]
///  6..8   sequence number, wrapping
///  8..12  timestamp: frames elapsed on the sender's sample clock
/// 12      sample rate marker, encoded as Scream does
/// 13      bits per sample
/// 14      channel count
/// 15      reserved, must be zero
/// 16..18  channel mask
/// 18..20  payload length in bytes
/// 20..    PCM payload
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonduitPacket<'a> {
    /// Format declared by the header.
    pub format: Format,
    /// Wrapping sequence number, incremented once per packet.
    pub sequence: u16,
    /// Frames elapsed on the sender's sample clock since the stream started.
    ///
    /// This is a sample count, not wall-clock time, which is exactly what a
    /// drift estimator needs: comparing it against frames actually consumed by
    /// the receiver measures the difference between the two sample clocks.
    pub timestamp_frames: u32,
    /// Flag bits. See [`FLAG_WIRED_LINK`]; the rest are reserved and zero.
    pub flags: u8,
    /// Raw interleaved little-endian PCM.
    pub pcm: &'a [u8],
}

impl SonduitPacket<'_> {
    /// Whether the sender declared a wired link.
    #[must_use]
    pub const fn wired_link(&self) -> bool {
        self.flags & FLAG_WIRED_LINK != 0
    }
}

impl<'a> SonduitPacket<'a> {
    /// Whether a datagram carries Sonduit's magic prefix.
    #[must_use]
    pub fn has_magic(datagram: &[u8]) -> bool {
        datagram.len() >= SONDUIT_MAGIC.len() && datagram[..SONDUIT_MAGIC.len()] == SONDUIT_MAGIC
    }

    /// Decode a Sonduit datagram.
    ///
    /// # Errors
    /// Returns [`Error::BadMagic`] when the prefix is wrong,
    /// [`Error::UnsupportedVersion`] for a version this build does not know,
    /// and [`Error::BadLength`] when the declared payload length does not match
    /// what actually arrived.
    pub fn decode(datagram: &'a [u8]) -> Result<Self, Error> {
        if datagram.len() < SONDUIT_HEADER_BYTES {
            return Err(Error::BadLength {
                expected: SONDUIT_HEADER_BYTES,
                actual: datagram.len(),
            });
        }
        if !Self::has_magic(datagram) {
            return Err(Error::BadMagic);
        }

        let version = datagram[4];
        if version != SONDUIT_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }

        let declared = u16::from_le_bytes([datagram[18], datagram[19]]) as usize;
        let actual = datagram.len() - SONDUIT_HEADER_BYTES;
        if declared != actual {
            return Err(Error::BadLength {
                expected: declared,
                actual,
            });
        }

        let format = Format {
            sample_rate: Format::rate_from_marker(datagram[12])?,
            bit_depth: BitDepth::from_bits(datagram[13])?,
            channels: datagram[14],
            channel_mask: u16::from_le_bytes([datagram[16], datagram[17]]),
        };
        format.validate()?;

        Ok(Self {
            format,
            sequence: u16::from_le_bytes([datagram[6], datagram[7]]),
            timestamp_frames: u32::from_le_bytes([
                datagram[8],
                datagram[9],
                datagram[10],
                datagram[11],
            ]),
            flags: datagram[5],
            pcm: &datagram[SONDUIT_HEADER_BYTES..],
        })
    }

    /// Bytes an encoded packet with this payload will occupy.
    #[must_use]
    pub const fn encoded_len(pcm_len: usize) -> usize {
        SONDUIT_HEADER_BYTES + pcm_len
    }

    /// Encode into `out`, which must be exactly [`Self::encoded_len`] long.
    ///
    /// # Errors
    /// Returns [`Error::BadLength`] when `out` is the wrong size or the payload
    /// exceeds what the length field can express, and propagates format
    /// failures.
    pub fn encode(&self, out: &mut [u8]) -> Result<(), Error> {
        let needed = Self::encoded_len(self.pcm.len());
        if out.len() != needed {
            return Err(Error::BadLength {
                expected: needed,
                actual: out.len(),
            });
        }
        if u16::try_from(self.pcm.len()).is_err() {
            return Err(Error::BadLength {
                expected: u16::MAX as usize,
                actual: self.pcm.len(),
            });
        }
        self.format.validate()?;

        out[..4].copy_from_slice(&SONDUIT_MAGIC);
        out[4] = SONDUIT_VERSION;
        out[5] = self.flags;
        out[6..8].copy_from_slice(&self.sequence.to_le_bytes());
        out[8..12].copy_from_slice(&self.timestamp_frames.to_le_bytes());
        out[12] = self.format.rate_marker()?;
        out[13] = self.format.bit_depth.bits();
        out[14] = self.format.channels;
        out[15] = 0;
        out[16..18].copy_from_slice(&self.format.channel_mask.to_le_bytes());
        #[allow(clippy::cast_possible_truncation)] // checked by try_from above
        out[18..20].copy_from_slice(&(self.pcm.len() as u16).to_le_bytes());
        out[SONDUIT_HEADER_BYTES..].copy_from_slice(self.pcm);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Vec<u8> {
        (0..PCM_PAYLOAD_BYTES).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn scream_round_trips() {
        let format = Format::stereo_48k();
        let pcm = payload();
        let mut buffer = vec![0_u8; SCREAM_PACKET_BYTES];

        ScreamPacket::encode(&format, &pcm, &mut buffer).unwrap();
        let decoded = ScreamPacket::decode(&buffer).unwrap();

        assert_eq!(decoded.format, format);
        assert_eq!(decoded.pcm, &pcm[..]);
    }

    #[test]
    fn scream_header_bytes_are_exactly_as_documented() {
        let format = Format::stereo_48k();
        let mut buffer = vec![0_u8; SCREAM_PACKET_BYTES];
        ScreamPacket::encode(&format, &payload(), &mut buffer).unwrap();

        assert_eq!(buffer[0], 0x01, "48 kHz marker");
        assert_eq!(buffer[1], 16, "bit depth");
        assert_eq!(buffer[2], 2, "channels");
        assert_eq!(buffer[3], 0x03, "mask low byte");
        assert_eq!(buffer[4], 0x00, "mask high byte");
        assert_eq!(buffer.len(), 1157);
    }

    #[test]
    fn scream_rejects_every_wrong_length() {
        for length in [0_usize, 5, 1156, 1158, 2000] {
            let datagram = vec![0_u8; length];
            assert!(
                ScreamPacket::decode(&datagram).is_err(),
                "length {length} should be rejected"
            );
        }
    }

    #[test]
    fn sonduit_round_trips() {
        let pcm = payload();
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 0xBEEF,
            timestamp_frames: 0x0123_4567,
            flags: 0,
            pcm: &pcm,
        };

        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();
        let decoded = SonduitPacket::decode(&buffer).unwrap();

        assert_eq!(decoded, packet);
        assert_eq!(buffer.len(), 1172);
    }

    #[test]
    fn sonduit_rejects_a_scream_datagram() {
        let format = Format::stereo_48k();
        let mut buffer = vec![0_u8; SCREAM_PACKET_BYTES];
        ScreamPacket::encode(&format, &payload(), &mut buffer).unwrap();

        assert!(matches!(
            SonduitPacket::decode(&buffer),
            Err(Error::BadMagic)
        ));
        assert!(!SonduitPacket::has_magic(&buffer));
    }

    #[test]
    fn a_sealed_packet_is_refused_rather_than_played_as_noise() {
        // The encrypted format shares this magic and differs only in the
        // version byte. A build that does not hold a key must refuse it here;
        // decoding the ciphertext as PCM would send it to the speakers.
        let pcm = payload();
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 1,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();
        buffer[4] = SONDUIT_VERSION_SEALED;

        assert!(matches!(
            SonduitPacket::decode(&buffer),
            Err(Error::UnsupportedVersion(SONDUIT_VERSION_SEALED))
        ));
        assert_ne!(SONDUIT_VERSION, SONDUIT_VERSION_SEALED);
    }

    #[test]
    fn sonduit_rejects_a_future_version() {
        let pcm = payload();
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 1,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();
        buffer[4] = SONDUIT_VERSION + 1;

        assert!(matches!(
            SonduitPacket::decode(&buffer),
            Err(Error::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn sonduit_rejects_a_lying_length_field() {
        let pcm = payload();
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 1,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();
        // Claim one byte more than actually follows the header.
        buffer[18..20].copy_from_slice(&((pcm.len() + 1) as u16).to_le_bytes());

        assert!(matches!(
            SonduitPacket::decode(&buffer),
            Err(Error::BadLength { .. })
        ));
    }

    #[test]
    fn sonduit_accepts_a_short_final_payload() {
        // A stream ending mid-packet still has to encode; the length field is
        // what makes that expressible at all, unlike Scream's fixed size.
        let pcm = vec![7_u8; 64];
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 9,
            timestamp_frames: 42,
            flags: 0,
            pcm: &pcm,
        };
        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();

        let decoded = SonduitPacket::decode(&buffer).unwrap();
        assert_eq!(decoded.pcm.len(), 64);
    }

    #[test]
    fn truncated_datagrams_never_panic() {
        let pcm = payload();
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 1,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut buffer = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut buffer).unwrap();

        for length in 0..SONDUIT_HEADER_BYTES + 4 {
            let _ = SonduitPacket::decode(&buffer[..length]);
            let _ = ScreamPacket::decode(&buffer[..length]);
        }
    }
}
