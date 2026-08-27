//! Device discovery.
//!
//! Deliberately minimal and self-contained: a UDP broadcast probe and reply,
//! scoped to one interface. ADR-006 records why this is preferred to mDNS for
//! the first milestone, and what would justify revisiting it.
//!
//! Scoping to one interface is not an optimisation. Over USB tethering the
//! phone hands out a random `/24` (see `docs/research/usb-transport.md`), so a
//! probe must go out of the tethered adapter specifically or it will be
//! answered by the wrong link, or by nothing.

use std::net::SocketAddr;

/// Magic prefix on every discovery datagram.
pub const DISCOVERY_MAGIC: [u8; 4] = *b"SDDS";

/// Discovery protocol version.
pub const DISCOVERY_VERSION: u8 = 1;

/// Port discovery probes are sent to.
pub const DISCOVERY_PORT: u16 = 4011;

/// Longest device name the protocol carries, in bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 63;

/// Kind of discovery datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Sender looking for receivers.
    Probe,
    /// Receiver announcing itself.
    Announce,
}

impl MessageKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Probe => 1,
            Self::Announce => 2,
        }
    }

    const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Probe),
            2 => Some(Self::Announce),
            _ => None,
        }
    }
}

/// A discovered receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Human-readable device name.
    pub name: String,
    /// Port the receiver is listening for audio on.
    pub audio_port: u16,
}

/// Encode a probe datagram.
#[must_use]
pub fn encode_probe() -> Vec<u8> {
    let mut out = Vec::with_capacity(6);
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(MessageKind::Probe.tag());
    out
}

/// Encode an announcement.
///
/// The name is truncated to [`MAX_NAME_BYTES`] on a character boundary, so a
/// long or multi-byte name can never produce invalid UTF-8 on the wire.
#[must_use]
pub fn encode_announce(name: &str, audio_port: u16) -> Vec<u8> {
    let mut end = name.len().min(MAX_NAME_BYTES);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];

    let mut out = Vec::with_capacity(9 + name.len());
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(MessageKind::Announce.tag());
    out.extend_from_slice(&audio_port.to_le_bytes());
    #[allow(clippy::cast_possible_truncation)] // bounded by MAX_NAME_BYTES
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out
}

/// Classify a discovery datagram, returning its kind if it is one.
#[must_use]
pub fn peek_kind(datagram: &[u8]) -> Option<MessageKind> {
    if datagram.len() < 6 || datagram[..4] != DISCOVERY_MAGIC {
        return None;
    }
    if datagram[4] != DISCOVERY_VERSION {
        return None;
    }
    MessageKind::from_tag(datagram[5])
}

/// Decode an announcement, or `None` when the datagram is not a well-formed
/// one.
#[must_use]
pub fn decode_announce(datagram: &[u8]) -> Option<Announcement> {
    if peek_kind(datagram)? != MessageKind::Announce {
        return None;
    }
    if datagram.len() < 9 {
        return None;
    }

    let audio_port = u16::from_le_bytes([datagram[6], datagram[7]]);
    let name_len = datagram[8] as usize;
    let name_bytes = datagram.get(9..9 + name_len)?;

    Some(Announcement {
        name: String::from_utf8(name_bytes.to_vec()).ok()?,
        audio_port,
    })
}

/// The address a receiver's audio stream should be sent to, combining the
/// address a datagram came from with the port it advertised.
#[must_use]
pub fn audio_address(from: SocketAddr, announcement: &Announcement) -> SocketAddr {
    SocketAddr::new(from.ip(), announcement.audio_port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn a_probe_round_trips() {
        let datagram = encode_probe();
        assert_eq!(peek_kind(&datagram), Some(MessageKind::Probe));
        assert_eq!(decode_announce(&datagram), None);
    }

    #[test]
    fn an_announcement_round_trips() {
        let datagram = encode_announce("Pixel 8", 4010);
        assert_eq!(peek_kind(&datagram), Some(MessageKind::Announce));
        assert_eq!(
            decode_announce(&datagram),
            Some(Announcement {
                name: "Pixel 8".to_string(),
                audio_port: 4010,
            })
        );
    }

    #[test]
    fn foreign_datagrams_are_ignored() {
        assert_eq!(peek_kind(b""), None);
        assert_eq!(peek_kind(b"XXXX\x01\x01"), None);
        // Right magic, wrong version.
        assert_eq!(peek_kind(b"SDDS\x02\x01"), None);
        // Right magic and version, unknown kind.
        assert_eq!(peek_kind(b"SDDS\x01\x09"), None);
    }

    #[test]
    fn a_truncated_announcement_is_rejected_rather_than_panicking() {
        let datagram = encode_announce("Pixel 8", 4010);
        for length in 0..datagram.len() {
            let _ = decode_announce(&datagram[..length]);
        }
        // A length byte promising more name than is present must not panic.
        let mut lying = encode_announce("ab", 4010);
        lying[8] = 200;
        assert_eq!(decode_announce(&lying), None);
    }

    #[test]
    fn an_overlong_name_is_truncated_on_a_character_boundary() {
        // Four-byte characters, so a naive byte cut would split one.
        let name = "\u{1F600}".repeat(40);
        let datagram = encode_announce(&name, 4010);
        let decoded = decode_announce(&datagram).expect("must stay valid utf-8");

        assert!(decoded.name.len() <= MAX_NAME_BYTES);
        assert!(name.starts_with(&decoded.name));
    }

    #[test]
    fn an_empty_name_is_allowed() {
        let datagram = encode_announce("", 4010);
        let decoded = decode_announce(&datagram).unwrap();
        assert_eq!(decoded.name, "");
        assert_eq!(decoded.audio_port, 4010);
    }

    #[test]
    fn the_audio_address_takes_the_ip_from_the_sender() {
        let from = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 42, 7, 1)), 4011);
        let announcement = Announcement {
            name: "phone".to_string(),
            audio_port: 4010,
        };
        let address = audio_address(from, &announcement);

        assert_eq!(address.ip(), from.ip());
        assert_eq!(address.port(), 4010, "port comes from the announcement");
    }
}
