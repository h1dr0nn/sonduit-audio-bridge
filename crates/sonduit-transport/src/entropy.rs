//! The one place key material is drawn from the operating system.
//!
//! # Why this is not `random_seed()`
//!
//! There are already two ad-hoc generators in this project: one on the phone
//! for pairing codes and one on the desktop for discovery nonces. Both mix a
//! clock reading with a stack address, and both are honest about what that is
//! worth -- a six-digit code is twenty bits whatever it is seeded from, so the
//! seed's quality is not the limit.
//!
//! **An X25519 private key is the opposite case.** Its entire strength is the
//! seed, and a seed an attacker can narrow to a few million candidates is a
//! private key an attacker can recompute. A clock-and-stack-address seed would
//! turn the whole of ADR-009 back into decoration, and it would look exactly
//! like the working version while doing it.
//!
//! So key material comes from the platform's cryptographic source and from
//! nothing else, and a failure to read it is an error the caller must handle.
//! There is deliberately no fallback: a fallback here is the bug.
//!
//! # Why this module exists at all
//!
//! [`crate::session::KeyExchange::from_seed`] takes a seed rather than reading
//! one, so that the exchange can be driven from a fixed seed in a test. That
//! leaves the question of where a real seed comes from, and the answer has to
//! be written down once. Two copies of it, one in the desktop and one in the
//! Android binding crate, is two places for the fallback above to be added
//! later by somebody in a hurry.

use crate::session::{SALT_BYTES, SEED_BYTES};

/// The system's random source could not be read.
///
/// Not recoverable and not worth retrying in a loop: on both platforms this
/// crate targets the source is a kernel facility that either works or the
/// process is in no state to be encrypting anything.
#[derive(Debug, thiserror::Error)]
#[error("the operating system's random source could not be read: {0}")]
pub struct EntropyError(String);

/// Fill `out` with cryptographically strong random bytes.
///
/// # Errors
/// Returns [`EntropyError`] when the platform source refuses. Callers must
/// treat that as "no session", never as "carry on unencrypted".
pub fn fill(out: &mut [u8]) -> Result<(), EntropyError> {
    getrandom::fill(out).map_err(|error| EntropyError(error.to_string()))
}

/// A seed for one ephemeral key pair.
///
/// # Errors
/// As [`fill`].
pub fn key_seed() -> Result<[u8; SEED_BYTES], EntropyError> {
    let mut seed = [0_u8; SEED_BYTES];
    fill(&mut seed)?;
    Ok(seed)
}

/// A salt for one stream, in one direction.
///
/// Fresh per stream is not a nicety. The packet counter that makes each nonce
/// unique restarts at zero when a stream does, so a repeated salt means a
/// repeated nonce under a key that has already been used, which under
/// ChaCha20-Poly1305 hands over the keystream and the authenticator's key
/// together. See [`crate::session::SessionSecret::audio_key`].
///
/// # Errors
/// As [`fill`].
pub fn stream_salt() -> Result<[u8; SALT_BYTES], EntropyError> {
    let mut salt = [0_u8; SALT_BYTES];
    fill(&mut salt)?;
    Ok(salt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_seeds_are_not_the_same_seed() {
        // Not a test of randomness, which cannot be tested from inside. It is
        // a test that something is being read at all: a stub returning zeroes
        // would pass every other test in this crate.
        let first = key_seed().expect("the platform has a random source");
        let second = key_seed().expect("the platform has a random source");
        assert_ne!(first, second);
        assert_ne!(first, [0_u8; SEED_BYTES]);
    }

    #[test]
    fn two_salts_are_not_the_same_salt() {
        let first = stream_salt().expect("the platform has a random source");
        let second = stream_salt().expect("the platform has a random source");
        assert_ne!(first, second);
    }

    #[test]
    fn a_zero_length_request_is_not_an_error() {
        // The loop inside getrandom is bounded by the slice; an empty one is a
        // legitimate no-op and must not be turned into a failure a caller has
        // to special-case.
        assert!(fill(&mut []).is_ok());
    }
}
