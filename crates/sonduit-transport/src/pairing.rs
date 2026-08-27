//! Pairing, so discovery cannot be answered by a stranger.
//!
//! # The problem this solves
//!
//! Discovery is a broadcast probe and whatever replies. Without pairing, any
//! device on the network can reply first and the desktop starts sending it
//! everything the machine is playing. On a home network that is a nuisance; on
//! a shared one it is an eavesdropping bug that leaves no trace on either
//! machine.
//!
//! # What it does and does not protect
//!
//! The phone shows a code, the user types it on the desktop, and the desktop
//! rejects any announcement that cannot prove it knows the same code. That
//! stops an unpaired device being selected. It does **not** encrypt the audio:
//! anyone who can see the traffic can still reconstruct it. Encryption is a
//! separate decision with its own cost, recorded in `docs/roadmap.md`.
//!
//! # Why HMAC and not a plain comparison
//!
//! Sending the code in the reply would put it on the wire in clear on every
//! probe, so a passive listener would learn it and could then answer future
//! probes itself. Instead the code keys an HMAC over the probe's nonce and the
//! contents of the reply. The nonce is fresh per probe, so a captured reply
//! cannot be replayed against the next one, and the tag covers the port and
//! name so neither can be altered in flight.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Bytes of nonce in a probe.
///
/// Sixteen random bytes make a repeat within one session impossible in
/// practice, which is what stops a captured reply being replayed.
pub const NONCE_BYTES: usize = 16;

/// Bytes of authentication tag in an announcement.
///
/// The full SHA-256 output. Truncating would save 16 bytes in a datagram that
/// is nowhere near any limit, in exchange for a weaker tag.
pub const TAG_BYTES: usize = 32;

/// Digits in a pairing code.
pub const CODE_DIGITS: usize = 6;

/// Domain separator, so a tag from this protocol cannot be replayed into
/// another one that happens to key an HMAC with the same code.
const CONTEXT: &[u8] = b"sonduit-pairing-v1";

type HmacSha256 = Hmac<Sha256>;

/// A pairing code, held by both ends of a session.
///
/// Stored as the digits the user sees rather than as a derived key, because
/// the phone has to display it and the desktop has to compare what was typed.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    digits: [u8; CODE_DIGITS],
}

impl PairingCode {
    /// Parse a code the user typed.
    ///
    /// Spaces and dashes are accepted and ignored: a six-digit code is
    /// naturally read aloud in groups, and refusing `123-456` would be
    /// pedantry the user pays for.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut digits = [0_u8; CODE_DIGITS];
        let mut count = 0;

        for character in text.chars() {
            if character.is_whitespace() || character == '-' {
                continue;
            }
            let value = character.to_digit(10)?;
            if count == CODE_DIGITS {
                return None;
            }
            digits[count] = value as u8;
            count += 1;
        }

        (count == CODE_DIGITS).then_some(Self { digits })
    }

    /// Build a code from a random seed.
    ///
    /// The caller supplies the randomness: this crate has no I/O and cannot
    /// read the system's entropy source, and a code generated from a
    /// predictable seed would be no protection at all.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        let mut digits = [0_u8; CODE_DIGITS];
        let mut value = seed;
        for digit in &mut digits {
            *digit = (value % 10) as u8;
            value /= 10;
        }
        Self { digits }
    }

    /// The code as the user sees it.
    #[must_use]
    pub fn to_display(&self) -> String {
        self.digits
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect()
    }

    fn key(&self) -> [u8; CODE_DIGITS] {
        self.digits
    }
}

impl core::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never print the digits. A pairing code in a log file is a pairing
        // code an attacker can read, and logs are copied into bug reports.
        f.write_str("PairingCode(******)")
    }
}

/// Compute the tag a receiver puts on its announcement.
///
/// `body` is the part of the datagram the tag protects: everything after the
/// header and before the tag itself. Covering it is what stops the port or the
/// name being rewritten in flight.
#[must_use]
pub fn tag(code: &PairingCode, nonce: &[u8; NONCE_BYTES], body: &[u8]) -> [u8; TAG_BYTES] {
    let mut mac = HmacSha256::new_from_slice(&code.key()).expect("hmac accepts any key length");
    mac.update(CONTEXT);
    mac.update(nonce);
    mac.update(body);

    let mut out = [0_u8; TAG_BYTES];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// Check a tag.
///
/// Uses the MAC's own verification rather than comparing byte slices, because
/// a short-circuiting comparison leaks how many bytes matched and that is
/// enough to forge a tag one byte at a time.
#[must_use]
pub fn verify(
    code: &PairingCode,
    nonce: &[u8; NONCE_BYTES],
    body: &[u8],
    candidate: &[u8],
) -> bool {
    if candidate.len() != TAG_BYTES {
        return false;
    }
    let mut mac = HmacSha256::new_from_slice(&code.key()).expect("hmac accepts any key length");
    mac.update(CONTEXT);
    mac.update(nonce);
    mac.update(body);
    mac.verify_slice(candidate).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> PairingCode {
        PairingCode::parse("482913").unwrap()
    }

    const NONCE: [u8; NONCE_BYTES] = [7; NONCE_BYTES];

    #[test]
    fn a_tag_verifies_against_the_same_code_and_nonce() {
        let body = b"Pixel 8:4010";
        let computed = tag(&code(), &NONCE, body);
        assert!(verify(&code(), &NONCE, body, &computed));
    }

    #[test]
    fn a_different_code_does_not_verify() {
        // The whole point: a device that does not know the code cannot answer.
        let body = b"Attacker:4010";
        let computed = tag(&PairingCode::parse("000000").unwrap(), &NONCE, body);
        assert!(!verify(&code(), &NONCE, body, &computed));
    }

    #[test]
    fn a_tag_from_one_probe_does_not_verify_against_another() {
        // Without this a captured reply could be replayed at every future
        // probe, and pairing would only have to be defeated once.
        let body = b"Pixel 8:4010";
        let computed = tag(&code(), &NONCE, body);

        let other_nonce = [9_u8; NONCE_BYTES];
        assert!(!verify(&code(), &other_nonce, body, &computed));
    }

    #[test]
    fn altering_the_body_invalidates_the_tag() {
        // The port is in the body. Rewriting it in flight would redirect the
        // audio to a port of the attacker's choosing on the paired device.
        let computed = tag(&code(), &NONCE, b"Pixel 8:4010");
        assert!(!verify(&code(), &NONCE, b"Pixel 8:9999", &computed));
        assert!(!verify(&code(), &NONCE, b"Pixel 9:4010", &computed));
    }

    #[test]
    fn a_tag_of_the_wrong_length_is_refused_rather_than_compared() {
        assert!(!verify(&code(), &NONCE, b"body", &[]));
        assert!(!verify(&code(), &NONCE, b"body", &[0_u8; 16]));
        assert!(!verify(&code(), &NONCE, b"body", &[0_u8; 64]));
    }

    #[test]
    fn a_code_reads_back_as_the_user_typed_it() {
        assert_eq!(code().to_display(), "482913");
    }

    #[test]
    fn separators_a_user_would_type_are_accepted() {
        // Six digits get read aloud in groups, and get typed that way.
        for text in ["482913", "482 913", "482-913", " 4 8 2 9 1 3 "] {
            assert_eq!(PairingCode::parse(text), Some(code()), "failed on {text:?}");
        }
    }

    #[test]
    fn a_code_of_the_wrong_length_is_refused() {
        assert_eq!(PairingCode::parse("48291"), None);
        assert_eq!(PairingCode::parse("4829133"), None);
        assert_eq!(PairingCode::parse(""), None);
    }

    #[test]
    fn non_digits_are_refused_rather_than_silently_dropped() {
        // Dropping them would make "4829a13" parse as a valid but different
        // code, and the user would see a pairing failure with no explanation.
        assert_eq!(PairingCode::parse("4829a1"), None);
        assert_eq!(PairingCode::parse("48291!"), None);
    }

    #[test]
    fn a_code_never_prints_its_digits() {
        // Logs get pasted into bug reports.
        let printed = format!("{:?}", code());
        assert!(!printed.contains("482913"), "leaked: {printed}");
    }

    #[test]
    fn different_seeds_produce_different_codes() {
        let first = PairingCode::from_seed(482_913);
        let second = PairingCode::from_seed(482_914);
        assert_ne!(first, second);
        assert_eq!(first.to_display().len(), CODE_DIGITS);
    }

    #[test]
    fn a_seed_larger_than_six_digits_still_produces_six() {
        let code = PairingCode::from_seed(u64::MAX);
        assert_eq!(code.to_display().len(), CODE_DIGITS);
    }
}
