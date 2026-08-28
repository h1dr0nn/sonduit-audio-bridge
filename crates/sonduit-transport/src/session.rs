//! Session keys, agreed once at pairing and used by every audio datagram.
//!
//! # Why the pairing code cannot be the key
//!
//! [`crate::pairing::PairingCode`] is six digits. That is 10^6 possibilities,
//! a shade under twenty bits, and the announcement puts an HMAC-SHA256 over a
//! known nonce and a largely known body on the wire. Anyone who captures one
//! pairing exchange therefore holds an offline verifier for the code: a laptop
//! walks the whole space in well under a second. The code is not a secret
//! against an eavesdropper who was present when it was used, and a session key
//! derived from it by any KDF would be no better -- the KDF would be walked
//! exactly as fast.
//!
//! So the code is not used as key material. It **authenticates an ephemeral
//! Diffie-Hellman**, and the audio key comes out of that.
//!
//! # What that buys, and what it does not
//!
//! | Attacker | Outcome |
//! | --- | --- |
//! | Records the pairing exchange and the whole session | Brute-forces the code in under a second and learns nothing about the audio. X25519 is not weakened by knowing the code, so the recording stays unreadable. |
//! | Sits in the middle during the pairing window | Must substitute its own public key and tag it with a code it does not have: one online guess in 10^6, against a code regenerated for the next window. |
//! | Learned an earlier code | An earlier code authenticates nothing now. Every pairing generates a new code and a fresh key pair. |
//!
//! Twenty bits online, single-shot, is the security level six-digit numeric
//! pairing has anywhere it is used; Bluetooth numeric comparison makes the
//! same trade. Twenty bits *offline* would be no security at all, and that is
//! the difference this module exists to make.
//!
//! # The exchange
//!
//! ```text
//!   desktop                                             phone
//!     |  probe carrying nonce N, or the same N in a QR    |
//!     | ------------------------------------------------> |
//!     |  announce: name, port, HMAC(code; N, body)         |
//!     | <------------------------------------------------- |
//!     |  key offer: PA, HMAC(code; N, PA)                  |
//!     | ------------------------------------------------> |
//!     |  key accept: PB, HMAC(code; N, PA, PB)             |
//!     | <------------------------------------------------- |
//! ```
//!
//! Both ends then hold `X25519(a, PB) == X25519(b, PA)` and derive the same
//! master secret from it. The offer and the accept are two extra datagrams
//! rather than fields inside the probe and the announcement because the QR
//! pairing path has no probe on the wire at all -- the invite is delivered
//! optically -- and one handshake that works on both paths is worth one round
//! trip that happens once per pairing.
//!
//! Nothing here does I/O. The caller supplies the random seed, exactly as
//! [`crate::pairing::PairingCode::from_seed`] requires, so that the exchange
//! can be driven from a fixed seed and asserted against known values. Outside
//! a test that seed must come from [`crate::entropy::key_seed`] and from
//! nowhere else: a key pair from a predictable seed is no protection at all,
//! and it looks exactly like one that is.

use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::pairing::{PairingCode, CODE_DIGITS, NONCE_BYTES};

/// Bytes in an X25519 public key.
pub const PUBLIC_KEY_BYTES: usize = 32;

/// Bytes of seed a key pair is built from.
pub const SEED_BYTES: usize = 32;

/// Bytes in the per-stream salt carried by every sealed datagram.
///
/// Eight, so that a sender restarting a stream under the same pairing gets a
/// fresh key rather than a fresh counter under the old one. See
/// [`SessionSecret::audio_key`].
pub const SALT_BYTES: usize = 8;

/// Bytes in a derived key.
pub const KEY_BYTES: usize = 32;

/// Domain separator for key derivation.
///
/// Distinct from the pairing HMAC's separator, so material from one
/// construction can never be read as an input to the other.
const CONTEXT: &[u8] = b"sonduit-session-v1";

/// Label separating the audio key from the feedback key.
const AUDIO_LABEL: &[u8] = b"sonduit-audio-v1";

/// Label for the reverse direction, so a report cannot be replayed as audio.
const FEEDBACK_LABEL: &[u8] = b"sonduit-feedback-v1";

/// One end's ephemeral contribution to the key agreement.
///
/// Consumed by [`KeyExchange::agree`]: a Diffie-Hellman secret that can be
/// used twice is a Diffie-Hellman secret that will be, and the second use is
/// the one nobody tests.
pub struct KeyExchange {
    secret: StaticSecret,
    public: [u8; PUBLIC_KEY_BYTES],
}

impl core::fmt::Debug for KeyExchange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The public half is safe to print; the private half never is.
        f.write_str("KeyExchange(secret hidden)")
    }
}

impl KeyExchange {
    /// Build a key pair from 32 bytes of caller-supplied randomness.
    ///
    /// The seed must come from the platform's cryptographic random source. A
    /// seed from anything else makes the whole exchange decorative.
    #[must_use]
    pub fn from_seed(seed: [u8; SEED_BYTES]) -> Self {
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }

    /// The public half, to be sent to the other end.
    #[must_use]
    pub const fn public_key(&self) -> [u8; PUBLIC_KEY_BYTES] {
        self.public
    }

    /// Complete the agreement against the other end's public key.
    ///
    /// `initiator` and `responder` are both public keys of the exchange in
    /// role order, and both go into the derivation. Binding them means a
    /// transcript rewritten in flight produces a different key on each end
    /// rather than a working session on an attacker's terms.
    ///
    /// Returns `None` when the peer's key is one of the small-order points,
    /// which force the shared secret to zero whatever the private key is.
    /// Accepting one would hand every attacker the same key.
    #[must_use]
    pub fn agree(
        self,
        peer_public: &[u8; PUBLIC_KEY_BYTES],
        nonce: &[u8; NONCE_BYTES],
        code: &PairingCode,
        initiator: &[u8; PUBLIC_KEY_BYTES],
        responder: &[u8; PUBLIC_KEY_BYTES],
    ) -> Option<SessionSecret> {
        let shared = self.secret.diffie_hellman(&PublicKey::from(*peer_public));
        if !shared.was_contributory() {
            return None;
        }

        // The code goes in as well as the Diffie-Hellman output. It adds no
        // real entropy -- twenty bits an attacker already has -- but it costs
        // nothing, and it means the key depends on everything the two ends
        // agreed on rather than only on the one value they computed.
        let mut ikm = [0_u8; KEY_BYTES + CODE_DIGITS];
        ikm[..KEY_BYTES].copy_from_slice(shared.as_bytes());
        ikm[KEY_BYTES..].copy_from_slice(&code.key_bytes());

        let mut info = [0_u8; 64 + 2 * PUBLIC_KEY_BYTES];
        let mut at = 0;
        for part in [CONTEXT, initiator.as_slice(), responder.as_slice()] {
            info[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }

        let mut master = [0_u8; KEY_BYTES];
        let derived = Hkdf::<Sha256>::new(Some(nonce), &ikm)
            .expand(&info[..at], &mut master)
            .is_ok();
        ikm.zeroize();

        derived.then_some(SessionSecret(master))
    }
}

/// The secret both ends hold after pairing.
///
/// Long-lived relative to a stream: one pairing, many sessions. It never
/// encrypts anything directly; [`SessionSecret::audio_key`] derives a fresh
/// key per stream from it.
#[derive(Clone, ZeroizeOnDrop)]
pub struct SessionSecret([u8; KEY_BYTES]);

impl core::fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Same rule as PairingCode: logs get copied into bug reports.
        f.write_str("SessionSecret(********)")
    }
}

impl SessionSecret {
    /// The key for audio flowing sender to receiver, for one stream.
    ///
    /// `salt` must be freshly random for each stream a sender starts. This is
    /// the whole reason the salt exists and it is not optional: the packet
    /// counter that makes each nonce unique restarts at zero with every
    /// stream, so a sender that stopped and started under one key would repeat
    /// its nonces, and a repeated nonce under ChaCha20-Poly1305 does not
    /// degrade gracefully -- it hands over the keystream and the
    /// authenticator's own key together. Making the key fresh per stream
    /// removes the question rather than answering it.
    #[must_use]
    pub fn audio_key(&self, salt: &[u8; SALT_BYTES]) -> SessionKey {
        self.derive(salt, AUDIO_LABEL)
    }

    /// The key for feedback reports flowing receiver to sender.
    ///
    /// A separate key for the reverse direction, so a report can never be
    /// replayed into the audio path or the other way round.
    #[must_use]
    pub fn feedback_key(&self, salt: &[u8; SALT_BYTES]) -> SessionKey {
        self.derive(salt, FEEDBACK_LABEL)
    }

    fn derive(&self, salt: &[u8; SALT_BYTES], label: &[u8]) -> SessionKey {
        let mut key = [0_u8; KEY_BYTES];
        // HKDF-Expand's only failure is asking for more than 255 hash blocks,
        // which 32 bytes is not. Handled rather than unwrapped because a
        // panic on the pairing path would take the app with it.
        if Hkdf::<Sha256>::new(Some(salt), &self.0)
            .expand(label, &mut key)
            .is_err()
        {
            key = [0_u8; KEY_BYTES];
        }
        SessionKey(key)
    }
}

/// A key for one stream in one direction.
#[derive(Clone, ZeroizeOnDrop)]
pub struct SessionKey([u8; KEY_BYTES]);

impl core::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SessionKey(********)")
    }
}

impl SessionKey {
    /// The raw key bytes, for handing to the cipher.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

/// A completed handshake, for tests elsewhere in the crate.
///
/// The real exchange rather than a hand-made secret: a test that fabricates
/// key material proves nothing about the path a session actually takes.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::{KeyExchange, SessionSecret, SEED_BYTES};
    use crate::pairing::{PairingCode, NONCE_BYTES};

    /// Both ends of one pairing, as the two sides would hold them.
    pub(crate) fn pair() -> (SessionSecret, SessionSecret) {
        let nonce = [0x5A; NONCE_BYTES];
        let code = PairingCode::parse("482913").expect("a six digit code");
        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());

        let one = desktop
            .agree(&pb, &nonce, &code, &pa, &pb)
            .expect("a contributory exchange");
        let two = phone
            .agree(&pa, &nonce, &code, &pa, &pb)
            .expect("a contributory exchange");
        (one, two)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: [u8; NONCE_BYTES] = [0x5A; NONCE_BYTES];

    fn code() -> PairingCode {
        PairingCode::parse("482913").unwrap()
    }

    fn pair() -> (SessionSecret, SessionSecret) {
        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());

        let one = desktop.agree(&pb, &NONCE, &code(), &pa, &pb).unwrap();
        let two = phone.agree(&pa, &NONCE, &code(), &pa, &pb).unwrap();
        (one, two)
    }

    #[test]
    fn both_ends_derive_the_same_key() {
        let (one, two) = pair();
        let salt = [9_u8; SALT_BYTES];
        assert_eq!(
            one.audio_key(&salt).as_bytes(),
            two.audio_key(&salt).as_bytes()
        );
    }

    #[test]
    fn a_different_pairing_code_derives_a_different_key() {
        // The code is not the strength of this, but it is bound into it: two
        // pairings differing only in the code must not share a key.
        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());

        let mine = desktop.agree(&pb, &NONCE, &code(), &pa, &pb).unwrap();
        let theirs = phone
            .agree(
                &pa,
                &NONCE,
                &PairingCode::parse("000000").unwrap(),
                &pa,
                &pb,
            )
            .unwrap();

        let salt = [9_u8; SALT_BYTES];
        assert_ne!(
            mine.audio_key(&salt).as_bytes(),
            theirs.audio_key(&salt).as_bytes()
        );
    }

    #[test]
    fn a_different_nonce_derives_a_different_key() {
        // Otherwise two pairings of the same two devices would share a key,
        // and a session recorded today would decrypt one recorded tomorrow.
        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());

        let first = desktop.agree(&pb, &NONCE, &code(), &pa, &pb).unwrap();
        let second = phone
            .agree(&pa, &[0x11; NONCE_BYTES], &code(), &pa, &pb)
            .unwrap();

        let salt = [9_u8; SALT_BYTES];
        assert_ne!(
            first.audio_key(&salt).as_bytes(),
            second.audio_key(&salt).as_bytes()
        );
    }

    #[test]
    fn a_rewritten_transcript_derives_a_different_key() {
        // An attacker who swaps a public key in flight must leave two ends
        // that cannot talk, not a session on its own terms.
        let desktop = KeyExchange::from_seed([1; SEED_BYTES]);
        let phone = KeyExchange::from_seed([2; SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());
        let forged = KeyExchange::from_seed([3; SEED_BYTES]).public_key();

        let honest = desktop.agree(&pb, &NONCE, &code(), &pa, &pb).unwrap();
        let confused = phone.agree(&pa, &NONCE, &code(), &pa, &forged).unwrap();

        let salt = [9_u8; SALT_BYTES];
        assert_ne!(
            honest.audio_key(&salt).as_bytes(),
            confused.audio_key(&salt).as_bytes()
        );
    }

    #[test]
    fn a_low_order_public_key_is_refused_rather_than_agreed() {
        // These points force the shared secret to zero whatever the private
        // key is. Accepting one would give every attacker the same key.
        let mut one = [0_u8; PUBLIC_KEY_BYTES];
        one[0] = 1;

        for point in [[0_u8; PUBLIC_KEY_BYTES], one] {
            let exchange = KeyExchange::from_seed([7; SEED_BYTES]);
            let public = exchange.public_key();
            assert!(
                exchange
                    .agree(&point, &NONCE, &code(), &public, &point)
                    .is_none(),
                "accepted a low-order point"
            );
        }
    }

    #[test]
    fn the_two_directions_use_different_keys() {
        // Otherwise a feedback report could be replayed into the audio path.
        let (secret, _) = pair();
        let salt = [9_u8; SALT_BYTES];
        assert_ne!(
            secret.audio_key(&salt).as_bytes(),
            secret.feedback_key(&salt).as_bytes()
        );
    }

    #[test]
    fn a_different_salt_derives_a_different_key() {
        // This is what makes a restarted stream safe: the counter goes back to
        // zero, so the key must not be the one the old counter ran under.
        let (secret, _) = pair();
        assert_ne!(
            secret.audio_key(&[1; SALT_BYTES]).as_bytes(),
            secret.audio_key(&[2; SALT_BYTES]).as_bytes()
        );
    }

    #[test]
    fn key_material_never_prints_itself() {
        let (secret, _) = pair();
        let key = secret.audio_key(&[9; SALT_BYTES]);

        for printed in [format!("{secret:?}"), format!("{key:?}")] {
            assert!(printed.contains('*'), "unexpected debug output: {printed}");
        }
        assert_eq!(
            format!("{:?}", KeyExchange::from_seed([1; SEED_BYTES])),
            "KeyExchange(secret hidden)"
        );
    }
}
