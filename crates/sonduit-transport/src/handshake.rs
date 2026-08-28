//! The two key-agreement datagrams, as each end has to use them.
//!
//! [`crate::discovery`] encodes and decodes the offer and the accept;
//! [`crate::session`] does the Diffie-Hellman and the derivation. What is left
//! is the order the two ends have to put those in, and that order is where a
//! key agreement is usually got wrong: the transcript has to be bound the same
//! way on both sides, the ephemeral secret has to be used exactly once, and
//! the responder has to derive from the public keys it *saw* rather than from
//! the ones it expected.
//!
//! Both ends of that live here, in one file, so they cannot drift apart. The
//! desktop's send path and the phone's receive path each call one of these two
//! and do their own socket work around it.
//!
//! ```text
//!   desktop (initiator)                          phone (responder)
//!     | Offer::new(seed, nonce, code)                             |
//!     | ---- Offer::datagram(), the SDDS key offer -------------> |
//!     |                                    answer(offer, nonce,   |
//!     |                                           code, seed)     |
//!     | <--- the SDDS key accept -------------------------------- |
//!     | Offer::accept(..) -> SessionSecret          SessionSecret |
//! ```
//!
//! Nothing here does I/O and nothing here reads a clock. The seed comes from
//! [`crate::entropy`] in the application and from a constant in a test, which
//! is the only way the exchange can be asserted against known values.
//!
//! # What the code is doing in here
//!
//! Every datagram is tagged with an HMAC keyed by the six-digit pairing code
//! and bound to the nonce of the exchange it belongs to. That is what an
//! active attacker has to forge to substitute its own public key, and it is
//! one online guess in a million against a code that is regenerated for the
//! next window. It is *not* what keeps the audio secret: that is the X25519,
//! and it survives the code being brute-forced afterwards. ADR-009 is written
//! around that distinction.

use crate::discovery;
use crate::pairing::{PairingCode, NONCE_BYTES};
use crate::session::{KeyExchange, SessionSecret, PUBLIC_KEY_BYTES, SEED_BYTES};

/// The initiator's half of the exchange: the desktop, which sends the audio.
///
/// Holds the ephemeral secret between the two datagrams and is consumed by
/// [`Offer::accept`], so the secret cannot be used for a second agreement. A
/// Diffie-Hellman secret that can be used twice is one that eventually will
/// be, and the second use is the one nobody tests.
pub struct Offer {
    exchange: KeyExchange,
    public: [u8; PUBLIC_KEY_BYTES],
    nonce: [u8; NONCE_BYTES],
    code: PairingCode,
}

impl core::fmt::Debug for Offer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The code redacts itself and the exchange hides its private half;
        // spelling that out here keeps a future field from leaking by default.
        f.write_str("Offer(secret hidden)")
    }
}

impl Offer {
    /// Begin an exchange authenticated by `code` and bound to `nonce`.
    ///
    /// `nonce` must be the nonce of the pairing this exchange belongs to: the
    /// one the probe carried, or the one the QR invite carried. A different
    /// nonce derives a different key on this side and the two ends simply
    /// cannot talk, which is the intended outcome of a rewritten transcript
    /// and a confusing one to debug if it happens by accident.
    ///
    /// `seed` must come from [`crate::entropy::key_seed`] outside a test.
    #[must_use]
    pub fn new(seed: [u8; SEED_BYTES], nonce: [u8; NONCE_BYTES], code: PairingCode) -> Self {
        let exchange = KeyExchange::from_seed(seed);
        let public = exchange.public_key();
        Self {
            exchange,
            public,
            nonce,
            code,
        }
    }

    /// The datagram to send to the responder.
    ///
    /// Built on demand rather than stored, because a caller that has to send
    /// it three times over an unreliable link should be sending the same
    /// bytes, and holding them would invite a caller to keep the vector and
    /// the state apart.
    #[must_use]
    pub fn datagram(&self) -> Vec<u8> {
        discovery::encode_key_offer(&self.public, &self.nonce, &self.code)
    }

    /// The public key this offer carries, for a caller that needs to log or
    /// compare it. It is not a secret; it went out on the wire.
    #[must_use]
    pub const fn public_key(&self) -> [u8; PUBLIC_KEY_BYTES] {
        self.public
    }

    /// Whether `datagram` is the accept belonging to this offer.
    ///
    /// [`Offer::accept`] consumes the exchange, which is what stops one
    /// ephemeral secret being used for two agreements. A caller reading a
    /// socket cannot know in advance which of the datagrams arriving is the
    /// one, and must not be forced to spend the exchange on the first
    /// plausible one: a single forged datagram would then be enough to make
    /// every pairing fail. This answers that question without spending
    /// anything.
    ///
    /// It is not a trust decision on its own -- it verifies the tag, which is
    /// the same check [`Offer::accept`] then makes again.
    #[must_use]
    pub fn is_our_accept(&self, datagram: &[u8]) -> bool {
        discovery::decode_key_accept(datagram, &self.public, &self.nonce, &self.code).is_some()
    }

    /// Complete the agreement from the responder's reply.
    ///
    /// Returns `None` for a datagram that is not a key accept, one whose tag
    /// does not verify against this exchange's nonce and code, and one whose
    /// public key is a small-order point. The three are deliberately not told
    /// apart: none of them is a peer this end may key a session from, and a
    /// caller that could distinguish them would be tempted to treat one of
    /// them as recoverable.
    #[must_use]
    pub fn accept(self, datagram: &[u8]) -> Option<SessionSecret> {
        let responder =
            discovery::decode_key_accept(datagram, &self.public, &self.nonce, &self.code)?;
        self.exchange.agree(
            &responder,
            &self.nonce,
            &self.code,
            &self.public,
            &responder,
        )
    }
}

/// The responder's half: answer an offer and derive the same secret.
///
/// One call, because the responder has nothing to remember between the two
/// datagrams. It generates its key pair, replies, and is finished.
///
/// Returns the datagram to send back and the secret to keep, or `None` when
/// the offer is not one this end may answer -- malformed, tagged with a code
/// it does not hold, bound to a nonce it did not issue, or carrying a
/// small-order public key.
///
/// `seed` must come from [`crate::entropy::key_seed`] outside a test.
#[must_use]
pub fn answer(
    offer: &[u8],
    nonce: &[u8; NONCE_BYTES],
    code: &PairingCode,
    seed: [u8; SEED_BYTES],
) -> Option<(Vec<u8>, SessionSecret)> {
    let initiator = discovery::decode_key_offer(offer, nonce, code)?;

    let exchange = KeyExchange::from_seed(seed);
    let responder = exchange.public_key();

    // Derived from the key that arrived, not from any expectation about it.
    // The accept's tag covers both halves, so an attacker that rewrites either
    // one leaves two ends holding different secrets rather than one session on
    // its terms.
    let secret = exchange.agree(&initiator, nonce, code, &initiator, &responder)?;
    let reply = discovery::encode_key_accept(&responder, &initiator, nonce, code);

    Some((reply, secret))
}

/// Whether a datagram is a key offer, cheaply and without authenticating it.
///
/// For a receive loop that has to route one datagram of four kinds before it
/// can decide what to do with it. Never a reason to trust the contents.
#[must_use]
pub fn is_key_offer(datagram: &[u8]) -> bool {
    discovery::peek_kind(datagram) == Some(discovery::MessageKind::KeyOffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SALT_BYTES;

    const NONCE: [u8; NONCE_BYTES] = [0x5A; NONCE_BYTES];
    const SALT: [u8; SALT_BYTES] = [0x11; SALT_BYTES];

    fn code() -> PairingCode {
        PairingCode::parse("482913").expect("a six digit code")
    }

    /// The whole exchange, as two ends holding the same code would run it.
    fn run(
        desktop_seed: u8,
        phone_seed: u8,
        phone_code: &PairingCode,
    ) -> (Option<SessionSecret>, Option<SessionSecret>) {
        let offer = Offer::new([desktop_seed; SEED_BYTES], NONCE, code());
        let Some((accept, phone_secret)) = answer(
            &offer.datagram(),
            &NONCE,
            phone_code,
            [phone_seed; SEED_BYTES],
        ) else {
            return (None, None);
        };
        (offer.accept(&accept), Some(phone_secret))
    }

    #[test]
    fn both_ends_of_an_honest_exchange_hold_the_same_key() {
        let (desktop, phone) = run(1, 2, &code());
        let desktop = desktop.expect("the accept verifies");
        let phone = phone.expect("the offer verifies");

        assert_eq!(
            desktop.audio_key(&SALT).as_bytes(),
            phone.audio_key(&SALT).as_bytes()
        );
    }

    #[test]
    fn a_responder_with_a_different_code_does_not_answer() {
        // The typed digits were wrong, or the device is not the one the user
        // is looking at. Either way it must not end up keyed.
        let (desktop, phone) = run(1, 2, &PairingCode::parse("000001").unwrap());
        assert!(desktop.is_none());
        assert!(phone.is_none());
    }

    #[test]
    fn an_accept_bound_to_another_nonce_is_refused() {
        // A captured accept from an earlier pairing. Every exchange has its
        // own nonce, so replaying one is the case this check exists for.
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        let (stale, _) = answer(
            &Offer::new([1; SEED_BYTES], [0x11; NONCE_BYTES], code()).datagram(),
            &[0x11; NONCE_BYTES],
            &code(),
            [2; SEED_BYTES],
        )
        .expect("the stale exchange itself is well formed");

        assert!(offer.accept(&stale).is_none());
    }

    #[test]
    fn an_accept_for_somebody_elses_offer_is_refused() {
        // The accept's tag covers the initiator's public key as well as the
        // responder's. An accept produced against a different offer therefore
        // fails here rather than producing a key one end alone can use.
        let mine = Offer::new([1; SEED_BYTES], NONCE, code());
        let theirs = Offer::new([3; SEED_BYTES], NONCE, code());

        let (accept, _) =
            answer(&theirs.datagram(), &NONCE, &code(), [2; SEED_BYTES]).expect("well formed");

        assert!(mine.accept(&accept).is_none());
    }

    #[test]
    fn junk_is_refused_at_both_ends_rather_than_parsed() {
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        for rubbish in [&b""[..], &b"SDDS"[..], &[0_u8; 200][..]] {
            assert!(answer(rubbish, &NONCE, &code(), [2; SEED_BYTES]).is_none());
            assert!(!is_key_offer(rubbish));
        }
        assert!(offer.accept(&[0_u8; 200]).is_none());
    }

    #[test]
    fn an_offer_is_recognised_by_its_kind_and_an_accept_is_not() {
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        let datagram = offer.datagram();
        assert!(is_key_offer(&datagram));

        let (accept, _) = answer(&datagram, &NONCE, &code(), [2; SEED_BYTES]).expect("well formed");
        assert!(!is_key_offer(&accept));
    }

    #[test]
    fn two_pairings_of_the_same_two_devices_do_not_share_a_key() {
        // Every pairing draws a fresh seed, so this is what actually happens.
        // If it did not hold, a session recorded today would decrypt one
        // recorded tomorrow.
        let (first, _) = run(1, 2, &code());
        let (second, _) = run(3, 4, &code());

        assert_ne!(
            first.unwrap().audio_key(&SALT).as_bytes(),
            second.unwrap().audio_key(&SALT).as_bytes()
        );
    }

    #[test]
    fn a_forged_accept_does_not_spend_the_exchange() {
        // A receive loop reads whatever arrives. One datagram from somebody
        // who does not hold the code must not be able to make every pairing
        // fail, which is what consuming the exchange on the first candidate
        // would do.
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        let (forged, _) = answer(
            &Offer::new([9; SEED_BYTES], NONCE, code()).datagram(),
            &NONCE,
            &code(),
            [8; SEED_BYTES],
        )
        .expect("well formed, for somebody else's offer");
        let (honest, _) =
            answer(&offer.datagram(), &NONCE, &code(), [2; SEED_BYTES]).expect("well formed");

        assert!(!offer.is_our_accept(&forged));
        assert!(!offer.is_our_accept(b"rubbish"));
        assert!(offer.is_our_accept(&honest));
        assert!(offer.accept(&honest).is_some());
    }

    #[test]
    fn an_offer_does_not_print_its_secret() {
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        assert_eq!(format!("{offer:?}"), "Offer(secret hidden)");
    }
}
