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
//!
//! # Authentication
//!
//! Every probe carries a fresh nonce and every announcement carries an HMAC
//! over it, keyed by the pairing code the user typed. A device that does not
//! know the code cannot produce a tag that verifies, so it cannot be selected.
//! See [`crate::pairing`] for what that does and does not protect.

use std::net::SocketAddr;

use crate::pairing::{self, PairingCode, NONCE_BYTES, TAG_BYTES};

/// Magic prefix on every discovery datagram.
pub const DISCOVERY_MAGIC: [u8; 4] = *b"SDDS";

/// Discovery protocol version.
///
/// Two, not one: version one had no nonce and no tag, so anything on the
/// network could answer a probe. An old sender talking to a new receiver would
/// be rejected on the version check, which is the correct outcome rather than
/// a silent downgrade to no authentication.
pub const DISCOVERY_VERSION: u8 = 2;

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

/// Header length: magic, version, kind.
const HEADER_BYTES: usize = 6;

/// Encode a probe datagram carrying `nonce`.
///
/// The nonce must be fresh for each scan. Reusing one lets a captured
/// announcement be replayed against a later probe, which would defeat pairing
/// after a single interception.
#[must_use]
pub fn encode_probe(nonce: &[u8; NONCE_BYTES]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + NONCE_BYTES);
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(MessageKind::Probe.tag());
    out.extend_from_slice(nonce);
    out
}

/// Read the nonce out of a probe, so the reply can be tagged against it.
#[must_use]
pub fn probe_nonce(datagram: &[u8]) -> Option<[u8; NONCE_BYTES]> {
    if peek_kind(datagram)? != MessageKind::Probe {
        return None;
    }
    let bytes = datagram.get(HEADER_BYTES..HEADER_BYTES + NONCE_BYTES)?;
    let mut nonce = [0_u8; NONCE_BYTES];
    nonce.copy_from_slice(bytes);
    Some(nonce)
}

/// Encode an announcement, authenticated against the probe's nonce.
///
/// The name is truncated to [`MAX_NAME_BYTES`] on a character boundary, so a
/// long or multi-byte name can never produce invalid UTF-8 on the wire.
#[must_use]
pub fn encode_announce(
    name: &str,
    audio_port: u16,
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Vec<u8> {
    let mut end = name.len().min(MAX_NAME_BYTES);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];

    // The body is what the tag covers: the port and the name. Excluding either
    // would let it be rewritten in flight.
    let mut body = Vec::with_capacity(3 + name.len());
    body.extend_from_slice(&audio_port.to_le_bytes());
    #[allow(clippy::cast_possible_truncation)] // bounded by MAX_NAME_BYTES
    body.push(name.len() as u8);
    body.extend_from_slice(name.as_bytes());

    let mut out = Vec::with_capacity(HEADER_BYTES + body.len() + TAG_BYTES);
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(MessageKind::Announce.tag());
    out.extend_from_slice(&body);
    out.extend_from_slice(&pairing::tag(code, nonce, &body));
    out
}

/// Classify a discovery datagram, returning its kind if it is one.
#[must_use]
pub fn peek_kind(datagram: &[u8]) -> Option<MessageKind> {
    if datagram.len() < HEADER_BYTES || datagram[..4] != DISCOVERY_MAGIC {
        return None;
    }
    if datagram[4] != DISCOVERY_VERSION {
        return None;
    }
    MessageKind::from_tag(datagram[5])
}

/// Decode an announcement whose tag verifies against `nonce` and `code`.
///
/// Returns `None` for a malformed datagram and for a well-formed one from a
/// device that does not know the code. The caller cannot tell the two apart,
/// and does not need to: neither is a device it may send audio to.
///
/// There is deliberately no unauthenticated variant. An API that offers one
/// gets called by accident, and the accident is silent.
#[must_use]
pub fn decode_announce(
    datagram: &[u8],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Option<Announcement> {
    if peek_kind(datagram)? != MessageKind::Announce {
        return None;
    }

    // Port, name length, at least one byte of tag territory.
    let body_start = HEADER_BYTES;
    if datagram.len() < body_start + 3 + TAG_BYTES {
        return None;
    }

    let audio_port = u16::from_le_bytes([datagram[body_start], datagram[body_start + 1]]);
    let name_len = datagram[body_start + 2] as usize;
    let body_end = body_start + 3 + name_len;

    let body = datagram.get(body_start..body_end)?;
    let tag = datagram.get(body_end..body_end + TAG_BYTES)?;

    // Verified before the name is parsed. Everything after this point is data
    // from a device that has proved it knows the code.
    if !pairing::verify(code, nonce, body, tag) {
        return None;
    }

    let name_bytes = &body[3..];
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

    const NONCE: [u8; NONCE_BYTES] = [0x5A; NONCE_BYTES];

    fn code() -> PairingCode {
        PairingCode::parse("482913").unwrap()
    }

    fn other_code() -> PairingCode {
        PairingCode::parse("000001").unwrap()
    }

    #[test]
    fn a_probe_round_trips_and_carries_its_nonce() {
        let datagram = encode_probe(&NONCE);
        assert_eq!(peek_kind(&datagram), Some(MessageKind::Probe));
        assert_eq!(probe_nonce(&datagram), Some(NONCE));
    }

    #[test]
    fn a_probe_is_not_mistaken_for_an_announcement() {
        let datagram = encode_probe(&NONCE);
        assert_eq!(decode_announce(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn an_announcement_round_trips_when_both_ends_share_the_code() {
        let datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        assert_eq!(peek_kind(&datagram), Some(MessageKind::Announce));
        assert_eq!(
            decode_announce(&datagram, &NONCE, &code()),
            Some(Announcement {
                name: "Pixel 8".to_string(),
                audio_port: 4010,
            })
        );
    }

    #[test]
    fn an_announcement_from_a_device_with_the_wrong_code_is_rejected() {
        // The entire reason pairing exists. Without this check the desktop
        // sends everything the machine is playing to whoever replied first.
        let datagram = encode_announce("Attacker", 4010, &NONCE, &other_code());
        assert_eq!(decode_announce(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn a_captured_announcement_cannot_be_replayed_at_the_next_probe() {
        // Otherwise pairing only has to be defeated once, on any network the
        // user has ever been on.
        let datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        let later_probe = [0x11; NONCE_BYTES];

        assert!(decode_announce(&datagram, &NONCE, &code()).is_some());
        assert_eq!(decode_announce(&datagram, &later_probe, &code()), None);
    }

    #[test]
    fn rewriting_the_port_in_flight_invalidates_the_announcement() {
        // The port decides where the audio goes on the paired device.
        let mut datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        datagram[HEADER_BYTES] ^= 0xFF;
        assert_eq!(decode_announce(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn rewriting_the_name_in_flight_invalidates_the_announcement() {
        // A name is what the user picks from. Rewriting it turns a real device
        // into a decoy the user chooses deliberately.
        let mut datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        let name_at = HEADER_BYTES + 3;
        datagram[name_at] = b'X';
        assert_eq!(decode_announce(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn tampering_with_the_tag_invalidates_the_announcement() {
        let mut datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        let last = datagram.len() - 1;
        datagram[last] ^= 0x01;
        assert_eq!(decode_announce(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn foreign_datagrams_are_ignored() {
        assert_eq!(peek_kind(b""), None);
        assert_eq!(peek_kind(b"XXXX\x02\x01"), None);
        // Right magic, wrong version. Version 1 had no authentication, so
        // accepting it would be a silent downgrade.
        assert_eq!(peek_kind(b"SDDS\x01\x01"), None);
        // Right magic and version, unknown kind.
        assert_eq!(peek_kind(b"SDDS\x02\x09"), None);
    }

    #[test]
    fn a_truncated_announcement_is_rejected_rather_than_panicking() {
        let datagram = encode_announce("Pixel 8", 4010, &NONCE, &code());
        for length in 0..datagram.len() {
            assert_eq!(
                decode_announce(&datagram[..length], &NONCE, &code()),
                None,
                "accepted a datagram truncated to {length} bytes"
            );
        }
    }

    #[test]
    fn a_name_length_longer_than_the_datagram_is_rejected() {
        let mut lying = encode_announce("ab", 4010, &NONCE, &code());
        lying[HEADER_BYTES + 2] = 200;
        assert_eq!(decode_announce(&lying, &NONCE, &code()), None);
    }

    #[test]
    fn a_truncated_probe_yields_no_nonce_rather_than_panicking() {
        let datagram = encode_probe(&NONCE);
        for length in 0..datagram.len() {
            assert_eq!(probe_nonce(&datagram[..length]), None);
        }
    }

    #[test]
    fn an_overlong_name_is_truncated_on_a_character_boundary() {
        // Four-byte characters, so a naive byte cut would split one.
        let name = "\u{1F600}".repeat(40);
        let datagram = encode_announce(&name, 4010, &NONCE, &code());
        let decoded = decode_announce(&datagram, &NONCE, &code()).expect("must stay valid utf-8");

        assert!(decoded.name.len() <= MAX_NAME_BYTES);
        assert!(name.starts_with(&decoded.name));
    }

    #[test]
    fn an_empty_name_is_allowed() {
        let datagram = encode_announce("", 4010, &NONCE, &code());
        let decoded = decode_announce(&datagram, &NONCE, &code()).unwrap();
        assert_eq!(decoded.name, "");
        assert_eq!(decoded.audio_port, 4010);
    }

    #[test]
    fn the_audio_address_takes_the_ip_from_the_sender() {
        // Never from the datagram: an address in the payload is an address an
        // attacker chooses, and it would redirect the stream to a third party.
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
