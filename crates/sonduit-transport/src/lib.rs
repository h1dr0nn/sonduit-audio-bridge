//! UDP transport and device discovery.
//!
//! One code path serves both links. USB tethering presents as an ordinary IP
//! interface, so "WiFi" and "USB" differ only in which local address the
//! socket binds to. There is deliberately no second implementation; see
//! ADR-004.

#![forbid(unsafe_code)]

pub mod discovery;
pub mod packetize;
pub mod pairing;
pub mod sink;
pub mod source;

use std::io;
use std::net::{SocketAddr, UdpSocket};

use sonduit_core::packet::{SonduitPacket, SCREAM_PACKET_BYTES, SONDUIT_HEADER_BYTES};

/// Largest datagram Sonduit will send or expect.
///
/// Both wire formats are far below any Ethernet or WiFi MTU, so datagrams are
/// never fragmented. Keeping it that way is a requirement, not an accident:
/// a fragmented UDP datagram is lost entirely if any fragment is lost.
pub const MAX_DATAGRAM_BYTES: usize = 1500;

/// Default port, inherited from Scream so an unmodified driver can be a sender.
pub const DEFAULT_PORT: u16 = 4010;

/// Default multicast group, likewise inherited from Scream.
pub const DEFAULT_MULTICAST_GROUP: [u8; 4] = [239, 255, 77, 77];

/// Transport failures.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Underlying socket error.
    #[error("socket error: {0}")]
    Io(#[from] io::Error),

    /// A datagram arrived that is neither wire format.
    #[error("datagram of {0} bytes matches no known wire format")]
    UnknownFormat(usize),

    /// Packet decoding failed.
    #[error(transparent)]
    Codec(#[from] sonduit_core::Error),
}

/// Which wire format a datagram is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Sonduit's native format, with sequence numbers and timestamps.
    Sonduit,
    /// Scream compatibility format, fixed size and headerless beyond format.
    Scream,
}

/// Classify a datagram without decoding it.
///
/// Magic is checked before length, because a Sonduit packet can legitimately
/// be 1157 bytes long and must not be mistaken for a Scream one.
#[must_use]
pub fn classify(datagram: &[u8]) -> Option<Wire> {
    if SonduitPacket::has_magic(datagram) && datagram.len() >= SONDUIT_HEADER_BYTES {
        return Some(Wire::Sonduit);
    }
    if datagram.len() == SCREAM_PACKET_BYTES {
        return Some(Wire::Scream);
    }
    None
}

/// Bind a UDP socket for sending.
///
/// `local` selects the interface, which is the entire difference between the
/// WiFi and USB paths. Binding `0.0.0.0:0` lets the routing table choose.
///
/// # Errors
/// Propagates the bind failure.
pub fn bind_sender(local: SocketAddr) -> Result<UdpSocket, TransportError> {
    let socket = UdpSocket::bind(local)?;
    Ok(socket)
}

/// Bind a UDP socket for receiving, joining `group` when it is multicast.
///
/// # Errors
/// Propagates bind or multicast-join failure.
pub fn bind_receiver(
    local: SocketAddr,
    group: Option<[u8; 4]>,
) -> Result<UdpSocket, TransportError> {
    let socket = UdpSocket::bind(local)?;
    if let Some(group) = group {
        socket.join_multicast_v4(&group.into(), &std::net::Ipv4Addr::UNSPECIFIED)?;
    }
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonduit_core::format::Format;
    use sonduit_core::format::PCM_PAYLOAD_BYTES;
    use sonduit_core::packet::ScreamPacket;

    #[test]
    fn scream_datagrams_are_classified_by_length() {
        let mut datagram = vec![0_u8; SCREAM_PACKET_BYTES];
        ScreamPacket::encode(
            &Format::stereo_48k(),
            &vec![0_u8; PCM_PAYLOAD_BYTES],
            &mut datagram,
        )
        .unwrap();
        assert_eq!(classify(&datagram), Some(Wire::Scream));
    }

    #[test]
    fn sonduit_datagrams_are_classified_by_magic() {
        let pcm = vec![0_u8; PCM_PAYLOAD_BYTES];
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 0,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut datagram = vec![0_u8; SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut datagram).unwrap();
        assert_eq!(classify(&datagram), Some(Wire::Sonduit));
    }

    #[test]
    fn magic_wins_over_length() {
        // A Sonduit packet sized exactly like a Scream one must not be read as
        // Scream, or its header would be decoded as PCM.
        let pcm = vec![0_u8; SCREAM_PACKET_BYTES - SONDUIT_HEADER_BYTES];
        let packet = SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 0,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut datagram = vec![0_u8; SCREAM_PACKET_BYTES];
        packet.encode(&mut datagram).unwrap();

        assert_eq!(datagram.len(), SCREAM_PACKET_BYTES);
        assert_eq!(classify(&datagram), Some(Wire::Sonduit));
    }

    #[test]
    fn junk_is_classified_as_nothing() {
        for length in [0_usize, 1, 20, 500, 1156, 1158] {
            assert_eq!(classify(&vec![0_u8; length]), None, "length {length}");
        }
    }

    #[test]
    fn both_wire_formats_fit_in_one_datagram() {
        const { assert!(SCREAM_PACKET_BYTES < MAX_DATAGRAM_BYTES) };
        const { assert!(SonduitPacket::encoded_len(PCM_PAYLOAD_BYTES) < MAX_DATAGRAM_BYTES) };
    }
}
