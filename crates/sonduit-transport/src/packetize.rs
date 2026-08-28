//! Turning a PCM byte stream into wire datagrams.
//!
//! Kept free of sockets so the framing rules can be tested without a network:
//! a capture callback hands over whatever the engine happened to have ready,
//! which is never a whole number of packets, so the leftover has to be carried
//! across calls. Getting that wrong produces audio that is subtly wrong rather
//! than obviously broken, which is the worst kind of bug to find later.

use sonduit_core::format::{Format, PCM_PAYLOAD_BYTES};
use sonduit_core::packet::{ScreamPacket, SonduitPacket, FLAG_WIRED_LINK};

use crate::sealed::Sealer;
use crate::{TransportError, Wire};

/// Frames a PCM stream into fixed-size datagrams.
///
/// Owns the sequence number and the frame timestamp, both of which must
/// advance by exactly one packet each, and the partial packet left over when a
/// capture block does not divide evenly.
#[derive(Debug)]
pub struct Packetizer {
    format: Format,
    wire: Wire,
    /// PCM that arrived but did not fill a packet.
    pending: Vec<u8>,
    sequence: u16,
    /// Frames sent so far, which is the timestamp of the next packet.
    ///
    /// Wraps deliberately: the receiver compares deltas, and a `u32` of frames
    /// at 48 kHz wraps after about 25 hours.
    timestamp_frames: u32,
    packets: u64,
    /// Header flag bits every packet carries. See [`Packetizer::on_wired_link`].
    flags: u8,
    /// Present when the stream is encrypted. Owns the packet counter, which is
    /// why it lives here rather than being handed in per packet.
    sealer: Option<Sealer>,
}

impl Packetizer {
    /// A packetizer for `format`, emitting `wire` datagrams in the clear.
    #[must_use]
    pub fn new(format: Format, wire: Wire) -> Self {
        Self {
            format,
            wire,
            pending: Vec::with_capacity(PCM_PAYLOAD_BYTES * 2),
            sequence: 0,
            timestamp_frames: 0,
            packets: 0,
            flags: 0,
            sealer: None,
        }
    }

    /// A packetizer that encrypts every datagram under `sealer`.
    ///
    /// The wire format is Sonduit's, at [`crate::sealed::SEALED_VERSION`]:
    /// Scream's header has no version field and nowhere to put a tag, so there
    /// is no sealed Scream and asking for one is a mistake rather than an
    /// option.
    ///
    /// The sealer is moved in because it owns the packet counter. One sealer
    /// serves one stream; starting a second stream needs a second sealer with
    /// a fresh salt, and that is the constraint that keeps the nonces unique.
    #[must_use]
    pub fn sealed(format: Format, sealer: Sealer) -> Self {
        Self {
            format,
            wire: Wire::Sonduit,
            pending: Vec::with_capacity(PCM_PAYLOAD_BYTES * 2),
            sequence: 0,
            timestamp_frames: 0,
            packets: 0,
            flags: 0,
            sealer: Some(sealer),
        }
    }

    /// Whether these datagrams are encrypted.
    #[must_use]
    pub const fn is_sealed(&self) -> bool {
        self.sealer.is_some()
    }

    /// Declare that these packets travel over a wired link.
    ///
    /// Only the sender knows: it chose the interface. The receiver sees an
    /// address and nothing more, and USB tethering does not have a reserved
    /// range to recognise. Telling it lets it hold ten milliseconds instead of
    /// thirty.
    ///
    /// Scream datagrams have nowhere to carry this, so on that wire it is
    /// accepted and dropped rather than refused: the choice of wire format is
    /// not the caller's reason for saying which link it is on.
    #[must_use]
    pub const fn on_wired_link(mut self, wired: bool) -> Self {
        self.flags = if wired { FLAG_WIRED_LINK } else { 0 };
        self
    }

    /// Bytes of PCM in one packet.
    #[must_use]
    pub const fn payload_bytes(&self) -> usize {
        PCM_PAYLOAD_BYTES
    }

    /// Packets emitted so far.
    #[must_use]
    pub const fn packets(&self) -> u64 {
        self.packets
    }

    /// PCM held back because it did not fill a packet.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    /// Sequence number the next packet will carry.
    #[must_use]
    pub const fn next_sequence(&self) -> u16 {
        self.sequence
    }

    /// Feed captured PCM, calling `emit` once per complete datagram.
    ///
    /// PCM that does not fill a packet is retained, so a caller may push
    /// arbitrary block sizes. `emit` returning an error stops the run
    /// immediately, and the packet that failed is not retried: resending it
    /// later would arrive behind audio that has already moved on.
    ///
    /// # Errors
    /// Propagates whatever `emit` returns, and reports encoding failures.
    pub fn push<F>(&mut self, pcm: &[u8], mut emit: F) -> Result<(), TransportError>
    where
        F: FnMut(&[u8]) -> Result<(), TransportError>,
    {
        self.pending.extend_from_slice(pcm);

        let mut datagram = vec![0_u8; self.datagram_bytes()];
        let mut consumed = 0;

        while self.pending.len() - consumed >= PCM_PAYLOAD_BYTES {
            let payload = &self.pending[consumed..consumed + PCM_PAYLOAD_BYTES];
            match (&mut self.sealer, self.wire) {
                (Some(sealer), _) => sealer.seal(
                    &self.format,
                    self.timestamp_frames,
                    self.flags,
                    payload,
                    &mut datagram,
                )?,
                (None, Wire::Sonduit) => SonduitPacket {
                    format: self.format,
                    sequence: self.sequence,
                    timestamp_frames: self.timestamp_frames,
                    flags: self.flags,
                    pcm: payload,
                }
                .encode(&mut datagram)?,
                (None, Wire::Scream) => {
                    ScreamPacket::encode(&self.format, payload, &mut datagram)?;
                }
            }

            consumed += PCM_PAYLOAD_BYTES;
            self.sequence = self.sequence.wrapping_add(1);
            self.timestamp_frames = self
                .timestamp_frames
                .wrapping_add(self.frames_per_packet() as u32);
            self.packets += 1;

            let result = emit(&datagram);
            if result.is_err() {
                self.pending.drain(..consumed);
                return result;
            }
        }

        self.pending.drain(..consumed);
        Ok(())
    }

    const fn frames_per_packet(&self) -> usize {
        PCM_PAYLOAD_BYTES / self.format.bytes_per_frame()
    }

    fn datagram_bytes(&self) -> usize {
        match (self.sealer.is_some(), self.wire) {
            (true, _) => Sealer::sealed_len(PCM_PAYLOAD_BYTES),
            (false, Wire::Sonduit) => SonduitPacket::encoded_len(PCM_PAYLOAD_BYTES),
            (false, Wire::Scream) => sonduit_core::packet::SCREAM_PACKET_BYTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonduit_core::packet::SONDUIT_HEADER_BYTES;

    fn collect(wire: Wire, blocks: &[usize]) -> (Vec<Vec<u8>>, Packetizer) {
        let mut packetizer = Packetizer::new(Format::stereo_48k(), wire);
        let mut sent = Vec::new();
        let mut value = 0_u8;
        for &size in blocks {
            let block: Vec<u8> = (0..size)
                .map(|_| {
                    value = value.wrapping_add(1);
                    value
                })
                .collect();
            packetizer
                .push(&block, |datagram| {
                    sent.push(datagram.to_vec());
                    Ok(())
                })
                .unwrap();
        }
        (sent, packetizer)
    }

    #[test]
    fn a_block_smaller_than_a_packet_emits_nothing_and_is_retained() {
        let (sent, packetizer) = collect(Wire::Sonduit, &[100]);

        assert!(sent.is_empty());
        assert_eq!(packetizer.pending_bytes(), 100);
    }

    #[test]
    fn pcm_split_across_blocks_is_rejoined_without_loss_or_duplication() {
        // This is the case a capture callback actually produces: the engine
        // hands over whatever it had ready, which is not a whole packet.
        let block = PCM_PAYLOAD_BYTES / 3;
        let (sent, packetizer) = collect(Wire::Sonduit, &[block, block, block, block]);

        assert_eq!(sent.len(), 1, "one packet from three thirds plus a spare");
        assert_eq!(packetizer.pending_bytes(), block);

        let emitted: usize = sent.iter().map(|d| d.len() - SONDUIT_HEADER_BYTES).sum();
        assert_eq!(emitted + packetizer.pending_bytes(), block * 4);
    }

    #[test]
    fn the_payload_bytes_come_through_in_order() {
        let mut packetizer = Packetizer::new(Format::stereo_48k(), Wire::Sonduit);
        let pcm: Vec<u8> = (0..PCM_PAYLOAD_BYTES * 2)
            .map(|index| (index % 251) as u8)
            .collect();

        let mut rejoined = Vec::new();
        packetizer
            .push(&pcm, |datagram| {
                rejoined.extend_from_slice(&datagram[SONDUIT_HEADER_BYTES..]);
                Ok(())
            })
            .unwrap();

        assert_eq!(rejoined, pcm, "reassembled payload must equal the input");
    }

    #[test]
    fn sequence_and_timestamp_advance_by_exactly_one_packet() {
        let mut packetizer = Packetizer::new(Format::stereo_48k(), Wire::Sonduit);
        let frames_per_packet = (PCM_PAYLOAD_BYTES / 4) as u32;

        let mut headers = Vec::new();
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 3], |datagram| {
                let packet = SonduitPacket::decode(datagram).unwrap();
                headers.push((packet.sequence, packet.timestamp_frames));
                Ok(())
            })
            .unwrap();

        assert_eq!(
            headers,
            vec![(0, 0), (1, frames_per_packet), (2, frames_per_packet * 2)]
        );
    }

    #[test]
    fn the_sequence_wraps_rather_than_overflowing() {
        // Sixteen bits at 48 kHz wraps in under fifteen minutes, so this is a
        // normal event during any real session, not an edge case.
        let mut packetizer = Packetizer::new(Format::stereo_48k(), Wire::Sonduit);
        for _ in 0..u16::MAX {
            packetizer
                .push(&vec![0_u8; PCM_PAYLOAD_BYTES], |_| Ok(()))
                .unwrap();
        }
        assert_eq!(packetizer.next_sequence(), u16::MAX);

        let mut wrapped = None;
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 2], |datagram| {
                let packet = SonduitPacket::decode(datagram).unwrap();
                wrapped.get_or_insert(packet.sequence);
                Ok(())
            })
            .unwrap();

        assert_eq!(wrapped, Some(u16::MAX));
        assert_eq!(packetizer.next_sequence(), 1);
    }

    #[test]
    fn scream_datagrams_are_the_fixed_size_the_protocol_requires() {
        let (sent, _) = collect(Wire::Scream, &[PCM_PAYLOAD_BYTES * 2]);

        assert_eq!(sent.len(), 2);
        for datagram in sent {
            assert_eq!(datagram.len(), sonduit_core::packet::SCREAM_PACKET_BYTES);
        }
    }

    #[test]
    fn a_failing_emit_stops_immediately_and_does_not_resend_the_failed_packet() {
        let mut packetizer = Packetizer::new(Format::stereo_48k(), Wire::Sonduit);
        let mut attempts = 0;

        let result = packetizer.push(&vec![0_u8; PCM_PAYLOAD_BYTES * 4], |_| {
            attempts += 1;
            if attempts == 2 {
                return Err(TransportError::Io(std::io::Error::other("link down")));
            }
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(attempts, 2, "stops at the failure, does not carry on");
        assert_eq!(packetizer.pending_bytes(), PCM_PAYLOAD_BYTES * 2);
        // The failed packet's sequence is spent. Reusing the number would make
        // the receiver see a duplicate; leaving the gap tells it the truth.
        assert_eq!(packetizer.next_sequence(), 2);
    }

    #[test]
    fn a_recovered_link_carries_on_from_the_next_sequence() {
        let mut packetizer = Packetizer::new(Format::stereo_48k(), Wire::Sonduit);
        let _ = packetizer.push(&vec![0_u8; PCM_PAYLOAD_BYTES], |_| {
            Err(TransportError::Io(std::io::Error::other("link down")))
        });

        let mut sequences = Vec::new();
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES], |datagram| {
                sequences.push(SonduitPacket::decode(datagram).unwrap().sequence);
                Ok(())
            })
            .unwrap();

        // The receiver sees a one-packet gap, which is exactly what happened.
        assert_eq!(sequences, vec![1]);
    }

    #[test]
    fn a_wired_link_is_declared_in_every_packet() {
        // The receiver sizes its buffer from this, and it may join at any
        // point, so it cannot be announced once at the start of the stream.
        let format = Format::stereo_48k();
        let mut packetizer = Packetizer::new(format, Wire::Sonduit).on_wired_link(true);

        let mut seen = 0;
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 3], |datagram| {
                let packet = SonduitPacket::decode(datagram).expect("a packet we just wrote");
                assert!(
                    packet.wired_link(),
                    "packet {seen} does not declare the link"
                );
                seen += 1;
                Ok(())
            })
            .expect("encoding cannot fail for a whole number of packets");

        assert_eq!(seen, 3);
    }

    #[test]
    fn a_sealed_stream_emits_sealed_datagrams_the_receiver_can_open() {
        use crate::sealed::{Opener, Sealer};

        let (secret, opener_secret) = crate::session::tests_support::pair();
        let mut packetizer = Packetizer::sealed(
            Format::stereo_48k(),
            Sealer::new(&secret, [0xC3; crate::session::SALT_BYTES]),
        );
        assert!(packetizer.is_sealed());

        let pcm: Vec<u8> = (0..PCM_PAYLOAD_BYTES * 3)
            .map(|index| (index % 251) as u8)
            .collect();

        let mut sent = Vec::new();
        packetizer
            .push(&pcm, |datagram| {
                sent.push(datagram.to_vec());
                Ok(())
            })
            .unwrap();

        assert_eq!(sent.len(), 3);

        let mut opener = Opener::new(opener_secret);
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        let mut rejoined = Vec::new();
        for (index, datagram) in sent.iter().enumerate() {
            assert_eq!(datagram.len(), Sealer::sealed_len(PCM_PAYLOAD_BYTES));
            let opened = opener.open(datagram, &mut out).expect("must open");
            assert_eq!(opened.counter, index as u64);
            assert_eq!(opened.sequence, index as u16);
            rejoined.extend_from_slice(opened.pcm);
        }

        assert_eq!(rejoined, pcm, "the audio did not survive the round trip");
    }

    #[test]
    fn a_sealed_stream_advances_the_timestamp_exactly_as_a_cleartext_one_does() {
        // Drift correction reads this. If sealing changed it, every figure
        // downstream would be measured against the wrong clock.
        use crate::sealed::{Opener, Sealer};

        let (secret, opener_secret) = crate::session::tests_support::pair();
        let frames_per_packet = (PCM_PAYLOAD_BYTES / 4) as u32;
        let mut packetizer = Packetizer::sealed(
            Format::stereo_48k(),
            Sealer::new(&secret, [1; crate::session::SALT_BYTES]),
        );

        let mut opener = Opener::new(opener_secret);
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        let mut headers = Vec::new();
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 3], |datagram| {
                let opened = opener.open(datagram, &mut out).expect("must open");
                headers.push((opened.sequence, opened.timestamp_frames));
                Ok(())
            })
            .unwrap();

        assert_eq!(
            headers,
            vec![(0, 0), (1, frames_per_packet), (2, frames_per_packet * 2)]
        );
    }

    #[test]
    fn the_link_flag_survives_sealing_and_is_authenticated() {
        use crate::sealed::{Opener, Sealer};

        let (secret, opener_secret) = crate::session::tests_support::pair();
        let mut packetizer = Packetizer::sealed(
            Format::stereo_48k(),
            Sealer::new(&secret, [2; crate::session::SALT_BYTES]),
        )
        .on_wired_link(true);

        let mut opener = Opener::new(opener_secret);
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES], |datagram| {
                let opened = opener.open(datagram, &mut out).expect("must open");
                assert!(opened.wired_link());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn the_counter_carries_across_a_link_change_rather_than_restarting() {
        // What a link migration does to the packetizer, and the reason it must
        // move the sealer rather than build a new one: the desktop swaps the
        // socket underneath a running session and carries the packetizer
        // through this builder. A counter that went back to zero here would
        // repeat every nonce of the stream so far, under the key it already
        // used, which is the one failure ChaCha20-Poly1305 does not degrade
        // gracefully from.
        use crate::sealed::{Opener, Sealer};

        let (secret, opener_secret) = crate::session::tests_support::pair();
        let format = Format::stereo_48k();
        let mut packetizer = Packetizer::sealed(
            format,
            Sealer::new(&secret, [0x0E; crate::session::SALT_BYTES]),
        );

        let mut opener = Opener::new(opener_secret);
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        let mut counters = Vec::new();

        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 2], |datagram| {
                counters.push(opener.open(datagram, &mut out).expect("must open").counter);
                Ok(())
            })
            .unwrap();

        // The migration: through the builder, exactly as  does.
        let mut packetizer = packetizer.on_wired_link(true);
        assert!(packetizer.is_sealed(), "the migration dropped the sealer");

        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES * 2], |datagram| {
                let opened = opener.open(datagram, &mut out).expect("must still open");
                assert!(opened.wired_link());
                counters.push(opened.counter);
                Ok(())
            })
            .unwrap();

        assert_eq!(counters, vec![0, 1, 2, 3]);
        assert_eq!(opener.rejected(), 0, "a replayed counter would show here");
    }

    #[test]
    fn saying_nothing_is_the_default() {
        // A receiver reads an unset flag as "not stated" and falls back to its
        // own guess, so the default must not claim a link it does not know.
        let format = Format::stereo_48k();
        let mut packetizer = Packetizer::new(format, Wire::Sonduit);

        packetizer
            .push(&vec![0_u8; PCM_PAYLOAD_BYTES], |datagram| {
                let packet = SonduitPacket::decode(datagram).expect("a packet we just wrote");
                assert!(!packet.wired_link());
                assert_eq!(packet.flags, 0, "no other flag bit may be set either");
                Ok(())
            })
            .expect("encoding cannot fail");
    }
}
