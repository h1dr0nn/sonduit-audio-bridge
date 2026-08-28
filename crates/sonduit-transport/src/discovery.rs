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
//!
//! # Key agreement
//!
//! Two further datagrams follow a verified announcement, [`encode_key_offer`]
//! and [`encode_key_accept`], and they are what turns pairing into something
//! the audio path can be keyed from. The pairing code is twenty bits and
//! cannot be a key; it authenticates an ephemeral Diffie-Hellman instead.
//! [`crate::session`] has the reasoning and the threat model.

use std::net::SocketAddr;

use crate::pairing::{self, PairingCode, NONCE_BYTES, TAG_BYTES};
use crate::session::PUBLIC_KEY_BYTES;

/// Magic prefix on every discovery datagram.
pub const DISCOVERY_MAGIC: [u8; 4] = *b"SDDS";

/// Discovery protocol version.
///
/// Three. Version one had no nonce and no tag, so anything on the network
/// could answer a probe. Version two fixed that but agreed no key, so the
/// audio that followed went out in the clear.
///
/// The bump is the whole compatibility story for encryption, and it is
/// deliberate that it lives here rather than on the audio packet. Two peers
/// settle whether they can encrypt **before any audio flows**, in an exchange
/// that already fails loudly: a peer speaking version two is not answered and
/// not selected, so the user is told no device was found instead of watching a
/// session connect and stay silent. [`foreign_version`] exists so the message
/// can say which of the two it was.
///
/// An old sender talking to a new receiver is rejected on the version check,
/// which is the correct outcome rather than a silent downgrade to unencrypted
/// audio.
pub const DISCOVERY_VERSION: u8 = 3;

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
    /// Sender offering its half of the session key agreement.
    KeyOffer,
    /// Receiver returning its half.
    KeyAccept,
}

impl MessageKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Probe => 1,
            Self::Announce => 2,
            Self::KeyOffer => 3,
            Self::KeyAccept => 4,
        }
    }

    const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Probe),
            2 => Some(Self::Announce),
            3 => Some(Self::KeyOffer),
            4 => Some(Self::KeyAccept),
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

/// The version of a Sonduit discovery datagram this build does not speak.
///
/// `None` for anything that is not a discovery datagram at all, and for one
/// this build does speak.
///
/// This exists so that a peer running an older build can be reported as an
/// older build. Without it the two cases -- nothing on the network, and a
/// phone that cannot encrypt -- look identical to the user, and the second one
/// has an answer: update it.
#[must_use]
pub fn foreign_version(datagram: &[u8]) -> Option<u8> {
    if datagram.len() < HEADER_BYTES || datagram[..4] != DISCOVERY_MAGIC {
        return None;
    }
    (datagram[4] != DISCOVERY_VERSION).then_some(datagram[4])
}

/// Encode the sender's half of the key agreement.
///
/// Sent after the announcement has verified, unicast to the address that
/// announcement came from. It is a separate datagram rather than a field in
/// the probe because the QR pairing path has no probe on the wire: the invite
/// is delivered optically and the phone answers it directly, so a public key
/// carried in the probe would never reach it. See [`crate::session`].
#[must_use]
pub fn encode_key_offer(
    public_key: &[u8; PUBLIC_KEY_BYTES],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Vec<u8> {
    let kind = MessageKind::KeyOffer.tag();
    let mut out = Vec::with_capacity(HEADER_BYTES + PUBLIC_KEY_BYTES + TAG_BYTES);
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(kind);
    out.extend_from_slice(public_key);
    out.extend_from_slice(&pairing::agreement_tag(kind, code, nonce, public_key));
    out
}

/// Read the sender's public key out of a key offer whose tag verifies.
///
/// `None` for a malformed datagram and for one from a device that does not
/// know the code, which the caller cannot and need not tell apart.
#[must_use]
pub fn decode_key_offer(
    datagram: &[u8],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Option<[u8; PUBLIC_KEY_BYTES]> {
    if peek_kind(datagram)? != MessageKind::KeyOffer {
        return None;
    }
    let body = datagram.get(HEADER_BYTES..HEADER_BYTES + PUBLIC_KEY_BYTES)?;
    let tag = datagram.get(HEADER_BYTES + PUBLIC_KEY_BYTES..)?;

    if !pairing::verify_agreement(MessageKind::KeyOffer.tag(), code, nonce, body, tag) {
        return None;
    }

    let mut public_key = [0_u8; PUBLIC_KEY_BYTES];
    public_key.copy_from_slice(body);
    Some(public_key)
}

/// Encode the receiver's half of the key agreement.
///
/// The offer's public key goes into the tag as well as this one. Binding both
/// halves is what stops a transcript being rewritten in flight: an attacker
/// who swaps one key leaves two ends that derive different keys and cannot
/// talk, rather than a working session on its terms.
#[must_use]
pub fn encode_key_accept(
    responder_public: &[u8; PUBLIC_KEY_BYTES],
    initiator_public: &[u8; PUBLIC_KEY_BYTES],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Vec<u8> {
    let kind = MessageKind::KeyAccept.tag();
    let mut body = [0_u8; 2 * PUBLIC_KEY_BYTES];
    body[..PUBLIC_KEY_BYTES].copy_from_slice(initiator_public);
    body[PUBLIC_KEY_BYTES..].copy_from_slice(responder_public);

    let mut out = Vec::with_capacity(HEADER_BYTES + PUBLIC_KEY_BYTES + TAG_BYTES);
    out.extend_from_slice(&DISCOVERY_MAGIC);
    out.push(DISCOVERY_VERSION);
    out.push(kind);
    out.extend_from_slice(responder_public);
    out.extend_from_slice(&pairing::agreement_tag(kind, code, nonce, &body));
    out
}

/// Read the receiver's public key out of a key accept whose tag verifies.
#[must_use]
pub fn decode_key_accept(
    datagram: &[u8],
    initiator_public: &[u8; PUBLIC_KEY_BYTES],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
) -> Option<[u8; PUBLIC_KEY_BYTES]> {
    if peek_kind(datagram)? != MessageKind::KeyAccept {
        return None;
    }
    let responder = datagram.get(HEADER_BYTES..HEADER_BYTES + PUBLIC_KEY_BYTES)?;
    let tag = datagram.get(HEADER_BYTES + PUBLIC_KEY_BYTES..)?;

    let mut body = [0_u8; 2 * PUBLIC_KEY_BYTES];
    body[..PUBLIC_KEY_BYTES].copy_from_slice(initiator_public);
    body[PUBLIC_KEY_BYTES..].copy_from_slice(responder);

    if !pairing::verify_agreement(MessageKind::KeyAccept.tag(), code, nonce, &body, tag) {
        return None;
    }

    let mut public_key = [0_u8; PUBLIC_KEY_BYTES];
    public_key.copy_from_slice(responder);
    Some(public_key)
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
        assert_eq!(peek_kind(b"XXXX\x03\x01"), None);
        // Right magic, wrong version. Version 1 had no authentication and
        // version 2 agreed no key, so accepting either would be a silent
        // downgrade -- of the pairing check or of the encryption.
        assert_eq!(peek_kind(b"SDDS\x01\x01"), None);
        assert_eq!(peek_kind(b"SDDS\x02\x01"), None);
        // Right magic and version, unknown kind.
        assert_eq!(peek_kind(b"SDDS\x03\x09"), None);
    }

    #[test]
    fn an_older_peer_is_reported_as_older_rather_than_as_nothing() {
        // "No devices found" and "that phone is too old to encrypt" are
        // different problems with different answers, and only one of them the
        // user can act on.
        assert_eq!(foreign_version(b"SDDS\x02\x02"), Some(2));
        assert_eq!(foreign_version(b"SDDS\x01\x02"), Some(1));
        assert_eq!(foreign_version(&encode_probe(&NONCE)), None);
        assert_eq!(foreign_version(b"XXXX\x02\x02"), None);
        assert_eq!(foreign_version(b""), None);
    }

    #[test]
    fn a_key_offer_round_trips_and_carries_the_public_key() {
        let public = [7_u8; PUBLIC_KEY_BYTES];
        let datagram = encode_key_offer(&public, &NONCE, &code());

        assert_eq!(peek_kind(&datagram), Some(MessageKind::KeyOffer));
        assert_eq!(decode_key_offer(&datagram, &NONCE, &code()), Some(public));
    }

    #[test]
    fn a_key_offer_from_a_device_with_the_wrong_code_is_rejected() {
        // Without this the key agreement would authenticate nothing and
        // anyone on the network could become the other end of it.
        let datagram = encode_key_offer(&[7; PUBLIC_KEY_BYTES], &NONCE, &other_code());
        assert_eq!(decode_key_offer(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn rewriting_the_public_key_in_flight_invalidates_the_offer() {
        // This is the man-in-the-middle attempt. The whole handshake rests on
        // it failing.
        let mut datagram = encode_key_offer(&[7; PUBLIC_KEY_BYTES], &NONCE, &code());
        datagram[HEADER_BYTES] ^= 0xFF;
        assert_eq!(decode_key_offer(&datagram, &NONCE, &code()), None);
    }

    #[test]
    fn a_key_offer_from_one_pairing_does_not_verify_against_another() {
        let datagram = encode_key_offer(&[7; PUBLIC_KEY_BYTES], &NONCE, &code());
        assert_eq!(
            decode_key_offer(&datagram, &[0x11; NONCE_BYTES], &code()),
            None
        );
    }

    #[test]
    fn a_key_accept_round_trips_and_binds_the_offer_it_answers() {
        let initiator = [7_u8; PUBLIC_KEY_BYTES];
        let responder = [9_u8; PUBLIC_KEY_BYTES];
        let datagram = encode_key_accept(&responder, &initiator, &NONCE, &code());

        assert_eq!(peek_kind(&datagram), Some(MessageKind::KeyAccept));
        assert_eq!(
            decode_key_accept(&datagram, &initiator, &NONCE, &code()),
            Some(responder)
        );

        // Verified against a different offer, the same accept must fail: that
        // is what stops half a transcript being reused.
        assert_eq!(
            decode_key_accept(&datagram, &[8; PUBLIC_KEY_BYTES], &NONCE, &code()),
            None
        );
    }

    #[test]
    fn an_offer_cannot_be_replayed_as_an_accept() {
        // The two messages carry a public key in the same place. The kind byte
        // is inside the tag so one cannot be turned into the other.
        let public = [7_u8; PUBLIC_KEY_BYTES];
        let mut offer = encode_key_offer(&public, &NONCE, &code());
        offer[5] = MessageKind::KeyAccept.tag();

        assert_eq!(decode_key_accept(&offer, &public, &NONCE, &code()), None);
    }

    #[test]
    fn an_announcement_tag_cannot_be_lifted_into_a_key_offer() {
        // Both are HMACs under the same code and nonce, and an announcement
        // body of the right length would otherwise carry a usable tag.
        let name = "x".repeat(PUBLIC_KEY_BYTES - 3);
        let announcement = encode_announce(&name, 4010, &NONCE, &code());

        let mut forged = Vec::with_capacity(announcement.len());
        forged.extend_from_slice(&DISCOVERY_MAGIC);
        forged.push(DISCOVERY_VERSION);
        forged.push(MessageKind::KeyOffer.tag());
        forged.extend_from_slice(&announcement[HEADER_BYTES..]);

        assert_eq!(decode_key_offer(&forged, &NONCE, &code()), None);
    }

    #[test]
    fn a_truncated_key_message_is_rejected_rather_than_panicking() {
        let public = [7_u8; PUBLIC_KEY_BYTES];
        let offer = encode_key_offer(&public, &NONCE, &code());
        let accept = encode_key_accept(&public, &public, &NONCE, &code());

        for length in 0..offer.len() {
            assert_eq!(decode_key_offer(&offer[..length], &NONCE, &code()), None);
        }
        for length in 0..accept.len() {
            assert_eq!(
                decode_key_accept(&accept[..length], &public, &NONCE, &code()),
                None
            );
        }
    }

    #[test]
    fn the_handshake_leaves_both_ends_holding_the_same_secret() {
        // The whole point, end to end over the four datagrams.
        use crate::session::{KeyExchange, SALT_BYTES, SEED_BYTES};

        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());

        let offer = encode_key_offer(&pa, &NONCE, &code());
        let seen_pa = decode_key_offer(&offer, &NONCE, &code()).expect("offer must verify");

        let accept = encode_key_accept(&pb, &seen_pa, &NONCE, &code());
        let seen_pb = decode_key_accept(&accept, &pa, &NONCE, &code()).expect("accept must verify");

        let desktop_secret = desktop
            .agree(&seen_pb, &NONCE, &code(), &pa, &seen_pb)
            .unwrap();
        let phone_secret = phone
            .agree(&seen_pa, &NONCE, &code(), &seen_pa, &pb)
            .unwrap();

        let salt = [3_u8; SALT_BYTES];
        assert_eq!(
            desktop_secret.audio_key(&salt).as_bytes(),
            phone_secret.audio_key(&salt).as_bytes()
        );
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
