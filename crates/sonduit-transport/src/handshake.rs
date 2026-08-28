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
//!     |                       Responder::answer(offer, nonces,    |
//!     |                                        code, seed)        |
//!     | <--- the SDDS key accept -------------------------------- |
//!     | Offer::accept(..) -> SessionSecret          SessionSecret |
//! ```
//!
//! Nothing here does I/O and nothing here reads a clock. The seed comes from
//! [`crate::entropy`] in the application and from a constant in a test, which
//! is the only way the exchange can be asserted against known values.
//!
//! # The offer is sent more than once, so answering it is stateful
//!
//! The initiator repeats its offer for loss tolerance and keeps the first
//! accept that comes back. That only agrees a key if a repeated offer is
//! answered the same way every time, so the responder is a [`Responder`] that
//! remembers what it answered rather than a function that answers afresh.
//! A responder that drew a new key pair per copy would leave the initiator
//! holding the first key it minted and itself holding the last, both ends
//! reporting a pairing, and every packet of the session refused.
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

/// How many answered offers a [`Responder`] remembers.
///
/// Four, the same depth the callers of this module keep of recent probe
/// nonces. One pairing is one remembered offer however many copies of it
/// arrive, so this is four *pairings* of history and not four datagrams. It is
/// what lets a duplicate that arrives after the user has re-paired still be
/// recognised as the repeat it is rather than answered afresh.
const REMEMBERED_OFFERS: usize = 4;

/// What a [`Responder`] decided about one key offer.
///
/// `secret` is `Some` only the first time an offer is answered. A repeat is
/// answered with the same `accept` bytes and no secret, because the responder
/// has already handed that secret to its caller and re-adopting it would let a
/// replayed old offer displace a newer pairing.
pub struct Answer {
    /// The key accept to send back to the initiator.
    pub accept: Vec<u8>,
    /// The master secret to adopt, or `None` when this is a repeat.
    pub secret: Option<SessionSecret>,
}

impl core::fmt::Debug for Answer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The accept went out on the wire; the secret never may.
        write!(
            f,
            "Answer({} bytes, {})",
            self.accept.len(),
            if self.secret.is_some() {
                "adopt"
            } else {
                "already held"
            }
        )
    }
}

/// The responder's half of the exchange: the phone, which plays the audio.
///
/// # Why this is not a free function
///
/// The initiator sends its offer more than once, because a single datagram
/// after an idle period is regularly dropped on a radio. A responder that
/// generated a fresh key pair per copy would answer three identical offers
/// with three different keys and keep the last, while the initiator kept the
/// first accept that reached it. Both ends would report a pairing, and every
/// packet of the session would then be refused.
///
/// So answering is stateful, and the state is here. An offer this responder
/// has already answered is answered again with **the same bytes**, which makes
/// the retransmission free: the initiator may keep any of the accepts it
/// receives, in any order, and holds the same secret either way.
///
/// # What "the same offer" means
///
/// The same initiator public key under the same nonce. Both are covered by the
/// offer's tag, so neither can be altered by anyone who does not hold the
/// pairing code, and a genuinely new exchange -- a fresh seed, or a fresh
/// pairing window -- differs in one or the other and is a miss.
///
/// # What an attacker gains from an answer that repeats
///
/// Nothing.
///
/// * Replaying a captured offer gets back the accept that was already on the
///   wire beside it. Both halves are public keys, so there is nothing in the
///   reply the replayer did not already hold.
/// * It buys no extra guess at the code. The memory is consulted only after an
///   offer's tag has verified, so a forged offer still costs one online guess
///   in 10^6 and a wrong one is still answered with silence.
/// * It reuses no Diffie-Hellman key across two exchanges. A hit is the same
///   exchange by definition; a different public key or a different nonce is a
///   miss and draws a fresh seed.
/// * It holds no key material. Only the public accept is remembered, so
///   nothing here widens what a compromise of this process yields, and
///   ADR-009's forward secrecy is untouched.
/// * A replayed *superseded* offer is answered but not adopted, so it cannot
///   knock out a pairing made since. Deriving a fresh key for it, which is
///   what a stateless responder does, can.
pub struct Responder {
    /// Offers answered, oldest first. Public bytes only.
    answered: Vec<Answered>,
}

/// One offer this responder has answered, and the reply it sent.
struct Answered {
    nonce: [u8; NONCE_BYTES],
    initiator: [u8; PUBLIC_KEY_BYTES],
    accept: Vec<u8>,
}

impl core::fmt::Debug for Responder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Responder({} answered)", self.answered.len())
    }
}

impl Default for Responder {
    fn default() -> Self {
        Self::new()
    }
}

impl Responder {
    /// A responder that has answered nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            answered: Vec::with_capacity(REMEMBERED_OFFERS),
        }
    }

    /// Answer `offer`, idempotently.
    ///
    /// `nonces` are the nonces of the pairings this end will answer for,
    /// newest last: the probes it has replied to lately, or the single nonce a
    /// scanned invite carried. The offer names none of them -- the tag is what
    /// decides which it belongs to -- so each is tried, newest first.
    ///
    /// Returns `None` for a datagram that is not a key offer, one whose tag
    /// verifies against none of `nonces`, and one carrying a small-order
    /// public key. The three are deliberately not told apart: none of them is
    /// a peer this end may key a session from, and a caller that could tell
    /// them apart would be tempted to treat one as recoverable.
    ///
    /// `seed` must come from [`crate::entropy::key_seed`] outside a test. It
    /// is not read at all for an offer already answered, which is the whole
    /// point of this type.
    #[must_use]
    pub fn answer(
        &mut self,
        offer: &[u8],
        nonces: &[[u8; NONCE_BYTES]],
        code: &PairingCode,
        seed: [u8; SEED_BYTES],
    ) -> Option<Answer> {
        for nonce in nonces.iter().rev() {
            let Some(initiator) = discovery::decode_key_offer(offer, nonce, code) else {
                continue;
            };

            // Verified, so this is the nonce the offer belongs to and no other
            // can be: the tag covers the nonce. Whatever happens below, there
            // is no point trying the rest.
            if let Some(seen) = self
                .answered
                .iter()
                .find(|seen| seen.nonce == *nonce && seen.initiator == initiator)
            {
                return Some(Answer {
                    accept: seen.accept.clone(),
                    secret: None,
                });
            }

            let exchange = KeyExchange::from_seed(seed);
            let responder = exchange.public_key();

            // Derived from the key that arrived, not from any expectation
            // about it. The accept's tag covers both halves, so an attacker
            // that rewrites either one leaves two ends holding different
            // secrets rather than one session on its terms.
            let secret = exchange.agree(&initiator, nonce, code, &initiator, &responder)?;
            let accept = discovery::encode_key_accept(&responder, &initiator, nonce, code);

            if self.answered.len() == REMEMBERED_OFFERS {
                self.answered.remove(0);
            }
            self.answered.push(Answered {
                nonce: *nonce,
                initiator,
                accept: accept.clone(),
            });

            return Some(Answer {
                accept,
                secret: Some(secret),
            });
        }

        None
    }
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

    /// One offer, answered by a responder that has answered nothing else.
    ///
    /// The tests that use it are about a single exchange. The ones about a
    /// repeated offer drive a [`Responder`] directly, because what it
    /// remembers between calls is the whole question there.
    fn answer(
        offer: &[u8],
        nonce: &[u8; NONCE_BYTES],
        code: &PairingCode,
        seed: [u8; SEED_BYTES],
    ) -> Option<(Vec<u8>, SessionSecret)> {
        let mut responder = Responder::new();
        let answered = responder.answer(offer, &[*nonce], code, seed)?;
        Some((
            answered.accept,
            answered.secret.expect("a first answer carries the secret"),
        ))
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
    fn a_repeated_offer_is_answered_with_the_same_bytes_and_one_key() {
        // The defect this type exists for. The initiator sends its offer three
        // times for loss tolerance and keeps whichever accept reaches it
        // first; a responder that answered each copy with a fresh key pair
        // left the two ends holding different secrets, a pairing both of them
        // called a success, and a session in which every packet was refused.
        //
        // A different seed per copy, because a responder that ignored the
        // repetition would be indistinguishable from a correct one under a
        // constant seed -- which is how this got past the tests.
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        let datagram = offer.datagram();
        let mut responder = Responder::new();

        let mut accepts = Vec::new();
        let mut phone_secret = None;
        for copy in 0..3_u8 {
            let answered = responder
                .answer(&datagram, &[NONCE], &code(), [0x40 + copy; SEED_BYTES])
                .expect("every copy of an offer this end can answer is answered");
            if copy == 0 {
                phone_secret = answered.secret.clone();
            } else {
                assert!(
                    answered.secret.is_none(),
                    "a repeat must not hand out a second secret to adopt"
                );
            }
            accepts.push(answered.accept);
        }

        assert_eq!(
            accepts[0], accepts[1],
            "the second copy answered differently"
        );
        assert_eq!(
            accepts[0], accepts[2],
            "the third copy answered differently"
        );

        // And the initiator agrees, whichever of the three it happened to keep.
        let phone = phone_secret.expect("the first answer carries the secret");
        let desktop = offer
            .accept(&accepts[2])
            .expect("the last accept must verify against the offer");
        assert_eq!(
            desktop.audio_key(&SALT).as_bytes(),
            phone.audio_key(&SALT).as_bytes(),
            "the two ends did not agree a key"
        );
    }

    #[test]
    fn a_new_exchange_is_not_answered_out_of_the_memory() {
        // The memory keys on the exchange, not on the peer. A second pairing
        // with the same device gets its own key pair, or two sessions would
        // share a secret.
        let mut responder = Responder::new();
        let first = Offer::new([1; SEED_BYTES], NONCE, code());
        let second = Offer::new([3; SEED_BYTES], NONCE, code());

        let one = responder
            .answer(&first.datagram(), &[NONCE], &code(), [0x40; SEED_BYTES])
            .expect("answered");
        let two = responder
            .answer(&second.datagram(), &[NONCE], &code(), [0x41; SEED_BYTES])
            .expect("answered");

        assert_ne!(one.accept, two.accept);
        let one = one.secret.expect("a first answer carries the secret");
        let two = two.secret.expect("a different exchange is a first answer");
        assert_ne!(
            one.audio_key(&SALT).as_bytes(),
            two.audio_key(&SALT).as_bytes()
        );
    }

    #[test]
    fn an_offer_a_later_pairing_replaced_is_answered_but_not_adopted() {
        // A duplicate of an old offer, delayed on the network or replayed by
        // somebody who captured it. It is answered, because the accept it gets
        // back is bytes it already saw; it must not become the current key
        // again, which would take down the pairing made since.
        let mut responder = Responder::new();
        let old = Offer::new([1; SEED_BYTES], NONCE, code()).datagram();
        let new = Offer::new([3; SEED_BYTES], NONCE, code()).datagram();

        let first = responder
            .answer(&old, &[NONCE], &code(), [0x40; SEED_BYTES])
            .expect("answered");
        let _ = responder
            .answer(&new, &[NONCE], &code(), [0x41; SEED_BYTES])
            .expect("answered");
        let replayed = responder
            .answer(&old, &[NONCE], &code(), [0x42; SEED_BYTES])
            .expect("answered");

        assert_eq!(replayed.accept, first.accept);
        assert!(
            replayed.secret.is_none(),
            "a replayed old offer must not displace the current pairing"
        );
    }

    #[test]
    fn an_offer_older_than_the_memory_is_answered_afresh() {
        // The memory is bounded, so this is what falling out of it looks like:
        // a new key pair and a secret to adopt. That is the same thing that
        // happens to an offer nobody remembers, and it is safe because the
        // initiator of an exchange that old is long gone.
        let mut responder = Responder::new();
        let first = Offer::new([1; SEED_BYTES], NONCE, code()).datagram();
        responder
            .answer(&first, &[NONCE], &code(), [0x40; SEED_BYTES])
            .expect("answered");
        for seed in 0..REMEMBERED_OFFERS as u8 {
            let other = Offer::new([0x80 + seed; SEED_BYTES], NONCE, code()).datagram();
            responder
                .answer(&other, &[NONCE], &code(), [0x50 + seed; SEED_BYTES])
                .expect("answered");
        }

        let again = responder
            .answer(&first, &[NONCE], &code(), [0x60; SEED_BYTES])
            .expect("answered");
        assert!(again.secret.is_some());
    }

    #[test]
    fn the_offer_is_tried_against_every_nonce_the_responder_still_holds() {
        // A responder answers probes from more than one desktop, and the offer
        // names none of the nonces it replied to. Newest first, but all of
        // them, or the second desktop to probe would pair and the first would
        // not.
        let mut responder = Responder::new();
        let older = [0x11_u8; NONCE_BYTES];
        let offer = Offer::new([1; SEED_BYTES], older, code());

        let answered = responder
            .answer(
                &offer.datagram(),
                &[older, NONCE],
                &code(),
                [0x40; SEED_BYTES],
            )
            .expect("the older nonce must still be tried");
        assert!(offer.accept(&answered.accept).is_some());
    }

    #[test]
    fn neither_the_responder_nor_its_answer_prints_a_secret() {
        let mut responder = Responder::new();
        assert_eq!(format!("{responder:?}"), "Responder(0 answered)");

        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        let answered = responder
            .answer(&offer.datagram(), &[NONCE], &code(), [0x40; SEED_BYTES])
            .expect("answered");
        assert_eq!(format!("{responder:?}"), "Responder(1 answered)");
        assert_eq!(
            format!("{answered:?}"),
            format!("Answer({} bytes, adopt)", answered.accept.len())
        );
    }

    #[test]
    fn an_offer_does_not_print_its_secret() {
        let offer = Offer::new([1; SEED_BYTES], NONCE, code());
        assert_eq!(format!("{offer:?}"), "Offer(secret hidden)");
    }
}
