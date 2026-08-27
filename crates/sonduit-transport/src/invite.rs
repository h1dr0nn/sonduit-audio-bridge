//! The pairing invite a desktop shows as a QR code.
//!
//! # Why this exists
//!
//! Discovery starts with a broadcast probe, and on a network where the two
//! devices sit in different subnets that probe never arrives: the phone on
//! 10.10.22.160/22 and the desktop on 10.10.0.61 cannot hear each other's
//! broadcasts and the scan finds nothing at all. The fallback was to read a
//! six-digit code off the phone and type the phone's address on the desktop,
//! which is two pieces of transcription for one connection.
//!
//! An invite turns that around. The desktop puts everything the phone needs on
//! screen at once, the phone's camera reads it, and the phone then sends its
//! announcement by unicast straight to an address it was given. Unicast
//! crosses a router; broadcast does not.
//!
//! # The invite is a probe delivered optically
//!
//! It carries the same nonce a broadcast probe would, so the announcement the
//! phone sends back is byte-for-byte the reply
//! [`crate::discovery::encode_announce`] already produces, authenticated the
//! same way against the same nonce. Nothing new is trusted and no second
//! authentication path exists to get wrong.
//!
//! # Threat model
//!
//! The code is on screen instead of on the phone, so the one threat that
//! changes is somebody photographing or shoulder-surfing the desktop's screen:
//! they learn the code and could announce themselves in the phone's place. That
//! was already true of the phone-side code the user reads aloud. Everything
//! else is unchanged, and in particular the code still never travels on the
//! wire, because the announcement carries an HMAC keyed by it rather than the
//! code itself.
//!
//! # Why this text encoding
//!
//! Every character is in the QR alphanumeric set (digits, `A`-`Z`, and
//! `: . -`), so encoders pack the payload at 5.5 bits per character instead of
//! the 8 that byte mode costs. A typical two-address invite is about 70
//! characters, which fits a version 4 symbol at medium error correction: large
//! modules, and a phone camera reads it across a desk rather than at arm's
//! length.

use std::net::Ipv4Addr;

use crate::pairing::{PairingCode, CODE_DIGITS, NONCE_BYTES};

/// Magic prefix and version of the invite format.
///
/// Versioned in the prefix rather than in a field, so a phone reading a future
/// invite fails the very first comparison instead of parsing three fields of a
/// format it does not know.
pub const INVITE_PREFIX: &str = "SDQ1";

/// Most addresses an invite will carry.
///
/// A machine with a VPN, a virtual switch and two real adapters can offer a
/// long list, and every extra address makes the QR denser and harder to scan
/// for the sake of an address the phone almost certainly cannot reach.
pub const MAX_INVITE_ADDRESSES: usize = 6;

/// Separator between the addresses.
///
/// A comma is the obvious choice and is not in the QR alphanumeric set, which
/// would push the whole payload into byte mode. A dash is, and cannot occur
/// inside an IPv4 address.
const ADDRESS_SEPARATOR: char = '-';

/// Separator between the fields.
const FIELD_SEPARATOR: char = ':';

/// Base32 alphabet, RFC 4648. Chosen over hexadecimal because every character
/// is still in the QR alphanumeric set while sixteen bytes cost 26 characters
/// instead of 32.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Characters a [`NONCE_BYTES`]-byte nonce occupies in base32, unpadded:
/// five bits per character, rounded up.
const NONCE_CHARS: usize = (NONCE_BYTES * 8).div_ceil(5);

/// What the desktop puts on screen and the phone reads back.
///
/// `Debug` is derived because [`PairingCode`] redacts itself; the nonce is not
/// a secret, since it is printed on the screen next to the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    /// Addresses the phone may send its announcement to, best first.
    ///
    /// Several, because the desktop cannot tell which of its interfaces the
    /// phone shares. The phone tries all of them; the ones on other links
    /// fail immediately and cost nothing.
    pub addresses: Vec<Ipv4Addr>,
    /// Port the desktop is listening for the announcement on.
    pub port: u16,
    /// The code that keys the announcement's tag.
    pub code: PairingCode,
    /// Freshness for the tag, exactly as a broadcast probe would carry.
    pub nonce: [u8; NONCE_BYTES],
}

impl Invite {
    /// Build an invite, keeping only addresses a phone could actually reach.
    ///
    /// Loopback, link-local, multicast and broadcast addresses are dropped
    /// rather than refused: they turn up in any real adapter list, and an
    /// invite that fails because the machine also has a `169.254` address
    /// would be a failure the user cannot act on.
    ///
    /// Returns `None` when nothing usable is left, because a QR code with no
    /// address in it is a QR code that cannot work and must not be shown.
    #[must_use]
    pub fn new(
        addresses: &[Ipv4Addr],
        port: u16,
        code: PairingCode,
        nonce: [u8; NONCE_BYTES],
    ) -> Option<Self> {
        if port == 0 {
            return None;
        }

        let mut kept: Vec<Ipv4Addr> = Vec::new();
        for address in addresses {
            if !is_reachable(*address) || kept.contains(address) {
                continue;
            }
            kept.push(*address);
            if kept.len() == MAX_INVITE_ADDRESSES {
                break;
            }
        }

        (!kept.is_empty()).then_some(Self {
            addresses: kept,
            port,
            code,
            nonce,
        })
    }

    /// The text to put in the QR code.
    #[must_use]
    pub fn to_payload(&self) -> String {
        let addresses = self
            .addresses
            .iter()
            .map(Ipv4Addr::to_string)
            .collect::<Vec<_>>()
            .join(&ADDRESS_SEPARATOR.to_string());

        format!(
            "{INVITE_PREFIX}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{addresses}",
            self.code.to_display(),
            self.port,
            encode_base32(&self.nonce),
        )
    }

    /// Read an invite out of scanned text.
    ///
    /// Deliberately strict. This text is produced by a machine and read by a
    /// camera, so anything that is not exactly the format is a scan of
    /// something else, and being lenient about it would mean starting a
    /// session against an address printed on somebody's business card.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut fields = text.trim().split(FIELD_SEPARATOR);

        let prefix = fields.next()?;
        if prefix != INVITE_PREFIX {
            return None;
        }

        let digits = fields.next()?;
        // Not PairingCode::parse alone: that forgives the spaces and dashes a
        // human types, and a dash here would mean the payload was cut across
        // the address separator.
        if digits.len() != CODE_DIGITS || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let code = PairingCode::parse(digits)?;

        let port: u16 = fields.next()?.parse().ok()?;
        if port == 0 {
            return None;
        }

        let nonce = decode_nonce(fields.next()?)?;

        let addresses = fields.next()?;
        if fields.next().is_some() {
            return None;
        }

        let mut parsed: Vec<Ipv4Addr> = Vec::new();
        for text in addresses.split(ADDRESS_SEPARATOR) {
            let address: Ipv4Addr = text.parse().ok()?;
            if !is_reachable(address) {
                return None;
            }
            if !parsed.contains(&address) {
                parsed.push(address);
            }
        }
        if parsed.is_empty() || parsed.len() > MAX_INVITE_ADDRESSES {
            return None;
        }

        Some(Self {
            addresses: parsed,
            port,
            code,
            nonce,
        })
    }
}

/// Whether an address is one a phone on some shared link could send to.
fn is_reachable(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_link_local())
}

/// Base32 without padding.
///
/// Padding would only add `=`, which is not in the QR alphanumeric set, to
/// encode a length both ends already know.
fn encode_base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;

    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(
                BASE32_ALPHABET[((buffer >> bits) & 0x1F) as usize],
            ));
        }
    }
    if bits > 0 {
        out.push(char::from(
            BASE32_ALPHABET[((buffer << (5 - bits)) & 0x1F) as usize],
        ));
    }

    out
}

/// Read a nonce back out of base32.
///
/// Rejects a non-canonical encoding, where the bits past the last whole byte
/// are not zero. Two encodings of one nonce would mean two payloads that pair
/// identically, and a format with slack in it is a format that drifts.
fn decode_nonce(text: &str) -> Option<[u8; NONCE_BYTES]> {
    if text.len() != NONCE_CHARS {
        return None;
    }

    let mut out = [0_u8; NONCE_BYTES];
    let mut written = 0_usize;
    let mut buffer = 0_u32;
    let mut bits = 0_u32;

    for byte in text.bytes() {
        let value = BASE32_ALPHABET.iter().position(|entry| *entry == byte)?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if written == NONCE_BYTES {
                return None;
            }
            out[written] = ((buffer >> bits) & 0xFF) as u8;
            written += 1;
        }
    }

    if written != NONCE_BYTES || buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DISCOVERY_PORT;

    const NONCE: [u8; NONCE_BYTES] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];

    fn code() -> PairingCode {
        PairingCode::parse("482913").unwrap()
    }

    fn invite() -> Invite {
        Invite::new(
            &[
                Ipv4Addr::new(10, 10, 0, 61),
                Ipv4Addr::new(192, 168, 42, 100),
            ],
            DISCOVERY_PORT,
            code(),
            NONCE,
        )
        .unwrap()
    }

    #[test]
    fn an_invite_round_trips_through_its_payload() {
        let original = invite();
        let parsed = Invite::parse(&original.to_payload()).expect("must parse what it printed");
        assert_eq!(parsed, original);
    }

    #[test]
    fn the_payload_is_only_characters_a_qr_encoder_packs_densely() {
        // Anything outside this set costs 8 bits per character instead of 5.5,
        // which is the difference between a symbol that scans across a desk
        // and one that has to be held up to the lens.
        const ALPHANUMERIC: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
        for character in invite().to_payload().chars() {
            assert!(
                ALPHANUMERIC.contains(character),
                "{character:?} forces byte mode"
            );
        }
    }

    #[test]
    fn a_two_address_invite_stays_small_enough_to_scan() {
        // Version 4 at medium error correction holds 114 alphanumeric
        // characters. Past that the modules shrink and so does the range.
        assert!(
            invite().to_payload().len() <= 114,
            "{}",
            invite().to_payload()
        );
    }

    #[test]
    fn even_a_full_address_list_fits_a_readable_symbol() {
        let addresses: Vec<Ipv4Addr> = (1..=MAX_INVITE_ADDRESSES)
            .map(|index| Ipv4Addr::new(192, 168, 100, 200 + index as u8))
            .collect();
        let full = Invite::new(&addresses, DISCOVERY_PORT, code(), NONCE).unwrap();
        // Version 6 at medium error correction, the practical ceiling.
        assert!(full.to_payload().len() <= 195, "{}", full.to_payload());
    }

    #[test]
    fn the_payload_never_contains_the_nonce_in_a_form_that_needs_padding() {
        assert!(!invite().to_payload().contains('='));
    }

    #[test]
    fn addresses_a_phone_cannot_reach_are_dropped_rather_than_offered() {
        // Every one of these turns up in a real Windows adapter list.
        let built = Invite::new(
            &[
                Ipv4Addr::LOCALHOST,
                Ipv4Addr::UNSPECIFIED,
                Ipv4Addr::new(169, 254, 3, 4),
                Ipv4Addr::new(239, 255, 77, 77),
                Ipv4Addr::BROADCAST,
                Ipv4Addr::new(10, 10, 0, 61),
            ],
            DISCOVERY_PORT,
            code(),
            NONCE,
        )
        .unwrap();

        assert_eq!(built.addresses, vec![Ipv4Addr::new(10, 10, 0, 61)]);
    }

    #[test]
    fn an_invite_with_no_usable_address_is_refused_rather_than_shown_empty() {
        // A QR the phone can read and do nothing with is worse than no QR.
        assert_eq!(
            Invite::new(&[Ipv4Addr::LOCALHOST], DISCOVERY_PORT, code(), NONCE),
            None
        );
        assert_eq!(Invite::new(&[], DISCOVERY_PORT, code(), NONCE), None);
    }

    #[test]
    fn a_duplicate_address_is_carried_once() {
        let built = Invite::new(
            &[Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(10, 0, 0, 2)],
            DISCOVERY_PORT,
            code(),
            NONCE,
        )
        .unwrap();
        assert_eq!(built.addresses.len(), 1);
    }

    #[test]
    fn the_address_list_is_capped() {
        let addresses: Vec<Ipv4Addr> = (1..=20)
            .map(|index| Ipv4Addr::new(10, 0, 0, index))
            .collect();
        let built = Invite::new(&addresses, DISCOVERY_PORT, code(), NONCE).unwrap();
        assert_eq!(built.addresses.len(), MAX_INVITE_ADDRESSES);
    }

    #[test]
    fn a_port_of_zero_is_refused() {
        // Zero means "any port" to a socket, and there is nothing to send to.
        assert_eq!(
            Invite::new(&[Ipv4Addr::new(10, 0, 0, 2)], 0, code(), NONCE),
            None
        );
        assert_eq!(
            Invite::parse("SDQ1:482913:0:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2"),
            None
        );
    }

    #[test]
    fn text_that_is_not_an_invite_is_refused() {
        // The camera sees whatever is in frame, including other people's QR
        // codes. None of them may start a session.
        for text in [
            "",
            "https://example.com",
            "SDQ0:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2:extra",
            "SDQ1:48291:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2",
            "SDQ1:4829133:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2",
            "SDQ1:48-913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2",
            "SDQ1:482913:70000:AAAAAAAAAAAAAAAAAAAAAAAAAA:10.0.0.2",
            "SDQ1:482913:4011:TOOSHORT:10.0.0.2",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAA1:10.0.0.2",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:not-an-address",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:127.0.0.1",
            "SDQ1:482913:4011:AAAAAAAAAAAAAAAAAAAAAAAAAA:",
        ] {
            assert_eq!(Invite::parse(text), None, "accepted {text:?}");
        }
    }

    #[test]
    fn a_non_canonical_nonce_is_refused() {
        // Sixteen bytes leave two spare bits in the last base32 character.
        // Setting them would give one nonce two payloads.
        let canonical = invite().to_payload();
        let slack = canonical.replace(&encode_base32(&NONCE), &{
            let mut altered = encode_base32(&NONCE);
            altered.pop();
            // 'B' is 1, so the two low bits carry a value the nonce does not.
            altered.push('B');
            altered
        });

        assert_ne!(slack, canonical);
        assert_eq!(Invite::parse(&slack), None);
    }

    #[test]
    fn the_example_in_the_protocol_document_parses() {
        // docs/protocol.md section 7 prints this. A document that shows an
        // invite the parser rejects is a document that teaches the wrong
        // format to whoever implements the next client.
        let parsed =
            Invite::parse("SDQ1:482913:4011:AEJEMZ4JVPHP73W3XKMHMVBSCA:10.10.0.61-192.168.42.100")
                .expect("the documented example must be a valid invite");

        assert_eq!(parsed.port, DISCOVERY_PORT);
        assert_eq!(parsed.code, code());
        assert_eq!(
            parsed.addresses,
            vec![
                Ipv4Addr::new(10, 10, 0, 61),
                Ipv4Addr::new(192, 168, 42, 100)
            ]
        );
    }

    #[test]
    fn surrounding_whitespace_from_a_scanner_is_forgiven() {
        assert!(Invite::parse(&format!("  {}\n", invite().to_payload())).is_some());
    }

    #[test]
    fn the_payload_does_not_leak_the_code_through_debug() {
        // Scanned payloads end up in logcat while this is being developed.
        assert!(!format!("{:?}", invite()).contains("482913"));
    }

    #[test]
    fn base32_round_trips_every_byte_value() {
        for value in 0..=u8::MAX {
            let nonce = [value; NONCE_BYTES];
            assert_eq!(decode_nonce(&encode_base32(&nonce)), Some(nonce));
        }
    }

    #[test]
    fn the_nonce_field_is_the_width_the_format_documents() {
        assert_eq!(NONCE_CHARS, 26);
        assert_eq!(encode_base32(&NONCE).len(), NONCE_CHARS);
    }

    #[test]
    fn an_invite_authenticates_the_announcement_the_phone_sends_back() {
        // The whole point: the QR is a probe, so the reply is the reply
        // discovery already knows how to verify, against the nonce the QR
        // carried rather than one a broadcast delivered.
        use crate::discovery::{decode_announce, encode_announce, Announcement};

        let shown = invite();
        let scanned = Invite::parse(&shown.to_payload()).unwrap();

        let reply = encode_announce("Pixel 7a", 4010, &scanned.nonce, &scanned.code);

        assert_eq!(
            decode_announce(&reply, &shown.nonce, &shown.code),
            Some(Announcement {
                name: "Pixel 7a".to_string(),
                audio_port: 4010,
            })
        );
    }

    #[test]
    fn an_announcement_keyed_by_another_invites_code_is_rejected() {
        use crate::discovery::{decode_announce, encode_announce};

        let shown = invite();
        let other = Invite::new(
            &[Ipv4Addr::new(10, 10, 0, 61)],
            DISCOVERY_PORT,
            PairingCode::parse("000001").unwrap(),
            NONCE,
        )
        .unwrap();

        let reply = encode_announce("Attacker", 4010, &other.nonce, &other.code);
        assert_eq!(decode_announce(&reply, &shown.nonce, &shown.code), None);
    }

    #[test]
    fn an_announcement_for_an_earlier_invite_cannot_be_replayed_at_the_next() {
        // Each invite carries a fresh nonce, so photographing yesterday's
        // screen is not enough to pair today.
        use crate::discovery::{decode_announce, encode_announce};

        let first = invite();
        let second =
            Invite::new(&first.addresses, first.port, code(), [0x77; NONCE_BYTES]).unwrap();

        let reply = encode_announce("Pixel 7a", 4010, &first.nonce, &first.code);
        assert!(decode_announce(&reply, &first.nonce, &first.code).is_some());
        assert_eq!(decode_announce(&reply, &second.nonce, &second.code), None);
    }
}
