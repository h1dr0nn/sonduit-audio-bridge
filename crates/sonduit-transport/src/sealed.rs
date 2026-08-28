//! The encrypted wire format, and the only place a key touches a datagram.
//!
//! # Shape
//!
//! A sealed audio datagram is a Sonduit packet with `version = 2`, a header
//! extended by twelve bytes, and a payload that is ChaCha20-Poly1305
//! ciphertext followed by its 16-byte tag:
//!
//! ```text
//!  0..4   magic "SDT1"
//!  4      version, 2 for sealed
//!  5      flags, as version 1
//!  6..8   packet counter, low 16 bits -- the sequence number the jitter
//!         buffer reads, unchanged in meaning
//!  8..12  timestamp: frames elapsed on the sender's sample clock
//! 12      sample rate marker
//! 13      bits per sample
//! 14      channel count
//! 15      reserved, must be zero
//! 16..18  channel mask
//! 18..20  plaintext length in bytes
//! 20..24  packet counter, high 32 bits
//! 24..32  stream salt
//! 32..    ciphertext, then the 16-byte tag
//! ```
//!
//! Bytes 0 to 20 keep the meaning they have in version 1, so anything that
//! reads a sequence number or a format for telemetry needs one reader and not
//! two. The whole 32-byte header is the AEAD's associated data, so every field
//! in it is authenticated even though none of it is hidden: rewriting the
//! sequence number, the timestamp, the declared format or the link flag makes
//! the tag fail.
//!
//! # Why the header is in the clear
//!
//! This is SRTP's arrangement and it is deliberate. The receiver has to route
//! a datagram, look up a key by salt and reject a replay before it can afford
//! to authenticate anything, and the jitter buffer needs the sequence number
//! whether or not the packet turns out to be good. What leaks is the sample
//! rate, the channel count and a frame counter, none of which says anything
//! about the audio; every datagram is the same size in either case, so the
//! traffic shape leaks no more than it did before.
//!
//! # Why the flag byte matters here
//!
//! `FLAG_WIRED_LINK` sizes the receiver's jitter buffer. Today's receiver
//! confirms a change over five consecutive packets because nothing
//! authenticates the wire, so a single injected packet must not be able to
//! retune the buffer. Under this format the flag is inside the AEAD's
//! associated data and only the paired sender can set it, which is what would
//! let that rule be simplified.
//!
//! # Nonces
//!
//! The nonce is the packet counter, little-endian in the low 8 bytes of the
//! 12-byte ChaCha20-Poly1305 nonce, and nothing else. It is never random and
//! never reused, which are the two things that matter:
//!
//! - **It does not wrap.** The `u16` sequence number on the wire wraps every
//!   393 seconds at 6 ms a packet, which is why the counter is not the
//!   sequence number: the high 32 bits ride in bytes 20..24, giving a 48-bit
//!   counter that reaches 53 million years before it repeats. The low 16 bits
//!   are the sequence number, so the two are the same value and the jitter
//!   buffer keeps working exactly as it does today.
//! - **It restarts only under a new key.** A counter that goes back to zero
//!   under an old key is the classic way to lose everything a stream cipher
//!   has; see [`crate::session::SessionSecret::audio_key`]. The stream salt in
//!   bytes 24..32 makes the key fresh for every stream a sender starts, so the
//!   counter may safely start at zero each time.
//!
//! Because the nonce is derived from a field the receiver already has, no
//! nonce is transmitted and no resynchronisation is needed: a receiver that
//! joins in the middle of a stream reads the salt and the counter out of the
//! first datagram it sees and is immediately in step.
//!
//! # Replay
//!
//! [`Opener`] keeps its own 256-packet sliding window. The jitter buffer also
//! rejects a packet that arrives behind the playback point, and for a running
//! session that would usually be enough, but it is not the same guarantee:
//! its window is tuned for latency and may be retuned tomorrow, it is disabled
//! while the buffer is still filling, and it lives in another crate at another
//! layer. A security property that holds only because of another component's
//! current tuning is a property that breaks silently. The window here costs
//! four `u64`s and two shifts.

use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, Tag};
use sonduit_core::format::{BitDepth, Format};
use sonduit_core::packet::SONDUIT_MAGIC;

use crate::feedback::{Feedback, FEEDBACK_BYTES};
use crate::session::{SessionSecret, SALT_BYTES};

/// Version byte a sealed audio datagram carries.
///
/// Version 1 is the cleartext format. An older receiver meeting this one
/// refuses it on the version check it already performs, rather than decoding
/// ciphertext as PCM and playing it as noise, which is what a flag bit in the
/// existing header would have caused. See ADR-009.
pub const SEALED_VERSION: u8 = 2;

/// Bytes of cleartext header on a sealed audio datagram.
pub const SEALED_HEADER_BYTES: usize = 32;

/// Bytes of Poly1305 tag.
pub const SEAL_TAG_BYTES: usize = 16;

/// Bytes a sealed datagram adds to its payload.
pub const SEALED_OVERHEAD_BYTES: usize = SEALED_HEADER_BYTES + SEAL_TAG_BYTES;

/// Largest packet counter the header can express.
///
/// Sixteen bits in the sequence field and thirty-two above it. At one packet
/// every six milliseconds this is reached after about 53 million years.
pub const MAX_COUNTER: u64 = (1 << 48) - 1;

/// Version byte a sealed feedback report carries.
pub const SEALED_FEEDBACK_VERSION: u8 = 2;

/// Bytes of cleartext header on a sealed feedback report.
pub const SEALED_FEEDBACK_HEADER_BYTES: usize = 22;

/// Encoded size of a sealed feedback report.
pub const SEALED_FEEDBACK_BYTES: usize =
    SEALED_FEEDBACK_HEADER_BYTES + FEEDBACK_BYTES + SEAL_TAG_BYTES;

/// Packets the replay window remembers.
///
/// 256 packets is about 1.5 seconds of audio, comfortably wider than any
/// jitter buffer this project configures, so the window never rejects a
/// packet the buffer would have been able to use.
const REPLAY_WINDOW_PACKETS: u64 = 256;

/// Failures on the sealed path.
///
/// A separate type from [`crate::TransportError`] rather than more variants on
/// it: a caller that must tell "forged" from "arrived late" apart should not
/// have to match it out of the same enum as a socket error.
#[derive(Debug, thiserror::Error)]
pub enum SealError {
    /// The datagram is too short to hold a header and a tag.
    #[error("a datagram of {0} bytes is too short to be a sealed packet")]
    TooShort(usize),

    /// The datagram does not carry Sonduit's magic prefix.
    #[error("not a Sonduit datagram")]
    BadMagic,

    /// The version is not one this build seals or opens.
    #[error("sealed packet version {0} is not one this build speaks")]
    UnsupportedVersion(u8),

    /// The declared length does not match what arrived, or a buffer is the
    /// wrong size.
    #[error("expected {expected} bytes, got {actual}")]
    BadLength {
        /// Length that was required.
        expected: usize,
        /// Length that was supplied.
        actual: usize,
    },

    /// The tag did not verify. The packet was forged, corrupted, or sent
    /// under a different key.
    ///
    /// The three are deliberately not distinguished. A receiver cannot tell
    /// them apart and does not need to: none of them is audio it may play.
    #[error("the packet did not authenticate")]
    NotAuthentic,

    /// The counter has been seen already, or is older than the window.
    #[error("packet {0} is a replay or is outside the replay window")]
    Replayed(u64),

    /// The counter reached [`MAX_COUNTER`]. Not reachable in this universe;
    /// refused rather than wrapped, because wrapping repeats a nonce.
    #[error("the packet counter is exhausted")]
    CounterExhausted,

    /// The authenticated header declared a format that is not representable.
    #[error(transparent)]
    Codec(#[from] sonduit_core::Error),
}

/// Nonce for a counter: little-endian in the low eight bytes, zero above.
fn nonce_for(counter: u64) -> Nonce {
    let mut bytes = [0_u8; 12];
    bytes[..8].copy_from_slice(&counter.to_le_bytes());
    Nonce::from(bytes)
}

fn cipher_for(secret: &SessionSecret, salt: &[u8; SALT_BYTES], audio: bool) -> ChaCha20Poly1305 {
    let key = if audio {
        secret.audio_key(salt)
    } else {
        secret.feedback_key(salt)
    };
    ChaCha20Poly1305::new(&Key::from(*key.as_bytes()))
}

/// A sliding window of counters already accepted.
///
/// The RFC 4303 construction: a highest-seen counter and a bitmap of the
/// window below it. Nothing is recorded until a packet has authenticated, so
/// a flood of forged counters cannot push the window forward and lock the
/// real sender out.
#[derive(Debug, Default)]
struct ReplayWindow {
    highest: u64,
    seen: [u64; 4],
    started: bool,
}

impl ReplayWindow {
    /// Whether `counter` could still be a packet worth authenticating.
    const fn admissible(&self, counter: u64) -> bool {
        if !self.started {
            return true;
        }
        if counter > self.highest {
            return true;
        }
        let behind = self.highest - counter;
        if behind >= REPLAY_WINDOW_PACKETS {
            return false;
        }
        self.seen[(behind / 64) as usize] & (1 << (behind % 64)) == 0
    }

    /// Record a counter that has authenticated.
    fn commit(&mut self, counter: u64) {
        if !self.started {
            self.started = true;
            self.highest = counter;
            self.seen = [0; 4];
            self.seen[0] = 1;
            return;
        }

        if counter > self.highest {
            shift_left(&mut self.seen, counter - self.highest);
            self.highest = counter;
            self.seen[0] |= 1;
        } else {
            let behind = self.highest - counter;
            if behind < REPLAY_WINDOW_PACKETS {
                self.seen[(behind / 64) as usize] |= 1 << (behind % 64);
            }
        }
    }

    /// Start again, for a stream running under a new key.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Shift a 256-bit window left, bit 0 being the newest.
fn shift_left(seen: &mut [u64; 4], amount: u64) {
    if amount >= REPLAY_WINDOW_PACKETS {
        *seen = [0; 4];
        return;
    }
    let words = (amount / 64) as usize;
    let bits = (amount % 64) as u32;

    let mut out = [0_u64; 4];
    for index in (words..4).rev() {
        let mut value = seen[index - words] << bits;
        if bits > 0 && index > words {
            value |= seen[index - words - 1] >> (64 - bits);
        }
        out[index] = value;
    }
    *seen = out;
}

/// Retired stream salts a receiver remembers.
///
/// A new salt means a new key and a counter starting again at zero, so the
/// replay window has to reset with it. That reset is a hole if a retired salt
/// is allowed to come back: somebody who recorded the stream before last could
/// replay it after the sender restarts, and every packet would authenticate,
/// because every packet of it was genuine. It is just not this stream.
///
/// Remembering the salts that have been retired closes that for 64 bytes. A
/// genuine salt never repeats: eight random bytes chosen per stream.
const RETIRED_SALTS: usize = 8;

/// Which stream a receiver is following, and which it has finished with.
struct StreamState {
    keyed: Option<([u8; SALT_BYTES], ChaCha20Poly1305)>,
    retired: [[u8; SALT_BYTES]; RETIRED_SALTS],
    retired_count: usize,
    next_retired: usize,
    replay: ReplayWindow,
}

impl StreamState {
    const fn new() -> Self {
        Self {
            keyed: None,
            retired: [[0; SALT_BYTES]; RETIRED_SALTS],
            retired_count: 0,
            next_retired: 0,
            replay: ReplayWindow {
                highest: 0,
                seen: [0; 4],
                started: false,
            },
        }
    }

    fn is_retired(&self, salt: &[u8; SALT_BYTES]) -> bool {
        self.retired[..self.retired_count].contains(salt)
    }

    fn cipher_for_salt(&self, salt: &[u8; SALT_BYTES]) -> Option<&ChaCha20Poly1305> {
        match &self.keyed {
            Some((seen, cipher)) if seen == salt => Some(cipher),
            _ => None,
        }
    }

    /// Follow a new stream, retiring the one before it.
    fn adopt(&mut self, salt: [u8; SALT_BYTES], cipher: ChaCha20Poly1305) {
        if let Some((previous, _)) = self.keyed.take() {
            self.retired[self.next_retired] = previous;
            self.next_retired = (self.next_retired + 1) % RETIRED_SALTS;
            self.retired_count = (self.retired_count + 1).min(RETIRED_SALTS);
        }
        self.keyed = Some((salt, cipher));
        self.replay.reset();
    }
}

/// Seals audio datagrams for one stream.
///
/// Owns the packet counter. That is the point of the type: a counter the
/// caller could set is a counter that will eventually be set twice, and a
/// nonce used twice under ChaCha20-Poly1305 loses both confidentiality and
/// the authenticator's key. There is deliberately no way to rewind it and no
/// `Clone`.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    salt: [u8; SALT_BYTES],
    counter: u64,
}

impl core::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sealer")
            .field("counter", &self.counter)
            .finish_non_exhaustive()
    }
}

impl Sealer {
    /// A sealer for one stream.
    ///
    /// `salt` must be freshly random per stream; see
    /// [`crate::session::SessionSecret::audio_key`] for what goes wrong if it
    /// is not.
    #[must_use]
    pub fn new(secret: &SessionSecret, salt: [u8; SALT_BYTES]) -> Self {
        Self {
            cipher: cipher_for(secret, &salt, true),
            salt,
            counter: 0,
        }
    }

    /// Bytes a sealed datagram with this payload will occupy.
    #[must_use]
    pub const fn sealed_len(pcm_len: usize) -> usize {
        SEALED_HEADER_BYTES + pcm_len + SEAL_TAG_BYTES
    }

    /// The counter the next packet will carry.
    #[must_use]
    pub const fn counter(&self) -> u64 {
        self.counter
    }

    /// The sequence number the next packet will carry, which is the counter's
    /// low sixteen bits.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // the low bits are the point
    pub const fn sequence(&self) -> u16 {
        self.counter as u16
    }

    /// The stream salt every datagram from this sealer carries.
    #[must_use]
    pub const fn salt(&self) -> [u8; SALT_BYTES] {
        self.salt
    }

    /// Seal one packet into `out`, which must be exactly
    /// [`Sealer::sealed_len`] long.
    ///
    /// Allocates nothing: the payload is copied into the caller's buffer and
    /// encrypted in place there, and the tag is written after it.
    ///
    /// # Errors
    /// Returns [`SealError::BadLength`] when `out` is the wrong size or the
    /// payload is longer than the length field can express,
    /// [`SealError::CounterExhausted`] at [`MAX_COUNTER`], and propagates a
    /// format that cannot be encoded.
    pub fn seal(
        &mut self,
        format: &Format,
        timestamp_frames: u32,
        flags: u8,
        pcm: &[u8],
        out: &mut [u8],
    ) -> Result<(), SealError> {
        let needed = Self::sealed_len(pcm.len());
        if out.len() != needed {
            return Err(SealError::BadLength {
                expected: needed,
                actual: out.len(),
            });
        }
        if u16::try_from(pcm.len()).is_err() {
            return Err(SealError::BadLength {
                expected: u16::MAX as usize,
                actual: pcm.len(),
            });
        }
        if self.counter > MAX_COUNTER {
            return Err(SealError::CounterExhausted);
        }
        format.validate()?;

        let counter = self.counter;
        #[allow(clippy::cast_possible_truncation)] // split deliberately
        let low = counter as u16;
        #[allow(clippy::cast_possible_truncation)] // bounded by MAX_COUNTER
        let high = (counter >> 16) as u32;

        out[0..4].copy_from_slice(&SONDUIT_MAGIC);
        out[4] = SEALED_VERSION;
        out[5] = flags;
        out[6..8].copy_from_slice(&low.to_le_bytes());
        out[8..12].copy_from_slice(&timestamp_frames.to_le_bytes());
        out[12] = format.rate_marker()?;
        out[13] = format.bit_depth.bits();
        out[14] = format.channels;
        out[15] = 0;
        out[16..18].copy_from_slice(&format.channel_mask.to_le_bytes());
        #[allow(clippy::cast_possible_truncation)] // checked by try_from above
        out[18..20].copy_from_slice(&(pcm.len() as u16).to_le_bytes());
        out[20..24].copy_from_slice(&high.to_le_bytes());
        out[24..32].copy_from_slice(&self.salt);

        let (header, body) = out.split_at_mut(SEALED_HEADER_BYTES);
        let (ciphertext, tag_bytes) = body.split_at_mut(pcm.len());
        ciphertext.copy_from_slice(pcm);

        let tag = self
            .cipher
            .encrypt_in_place_detached(&nonce_for(counter), header, ciphertext)
            .map_err(|_| SealError::NotAuthentic)?;
        tag_bytes.copy_from_slice(&tag);

        self.counter += 1;
        Ok(())
    }
}

/// A packet that authenticated.
///
/// The same fields [`sonduit_core::packet::SonduitPacket`] exposes, so the
/// receive path reads one shape whichever wire format arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened<'a> {
    /// Format declared by the authenticated header.
    pub format: Format,
    /// Sequence number, the counter's low sixteen bits.
    pub sequence: u16,
    /// The full packet counter, which is also the nonce.
    pub counter: u64,
    /// Frames elapsed on the sender's sample clock.
    pub timestamp_frames: u32,
    /// Flag bits, authenticated.
    pub flags: u8,
    /// The recovered PCM.
    pub pcm: &'a [u8],
}

impl Opened<'_> {
    /// Whether the sender declared a wired link.
    ///
    /// Unlike the version 1 flag, this one is authenticated: only the paired
    /// sender can set it.
    #[must_use]
    pub const fn wired_link(&self) -> bool {
        self.flags & sonduit_core::packet::FLAG_WIRED_LINK != 0
    }
}

/// Opens audio datagrams from one paired sender.
///
/// Holds the master secret rather than a key, because the key changes with
/// the stream salt and the receiver learns the salt from the packets.
pub struct Opener {
    secret: SessionSecret,
    stream: StreamState,
    rejected: u64,
}

impl core::fmt::Debug for Opener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Opener")
            .field("rejected", &self.rejected)
            .finish_non_exhaustive()
    }
}

impl Opener {
    /// An opener for a session, with nothing accepted yet.
    #[must_use]
    pub const fn new(secret: SessionSecret) -> Self {
        Self {
            secret,
            stream: StreamState::new(),
            rejected: 0,
        }
    }

    /// Datagrams refused since the session began.
    ///
    /// Forged, corrupted, replayed and mis-keyed packets all count here.
    /// Worth showing: on a healthy link it is zero, and anything else is
    /// either a bug or somebody on the network.
    #[must_use]
    pub const fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Whether a datagram claims to be a sealed Sonduit packet.
    ///
    /// Cheap and does not authenticate anything. Use it to route, never to
    /// decide whether to trust.
    #[must_use]
    pub fn is_sealed(datagram: &[u8]) -> bool {
        datagram.len() > SONDUIT_MAGIC.len()
            && datagram[..SONDUIT_MAGIC.len()] == SONDUIT_MAGIC
            && datagram[4] == SEALED_VERSION
    }

    /// Authenticate and decrypt one datagram into `out`.
    ///
    /// `out` must be a buffer the caller reuses; nothing here allocates. The
    /// returned [`Opened`] borrows the plaintext from it.
    ///
    /// # Errors
    /// Every failure mode is an error rather than a `None`, because the
    /// receiver's telemetry has to be able to tell "nothing arrived" from
    /// "something arrived and did not authenticate".
    pub fn open<'a>(
        &mut self,
        datagram: &[u8],
        out: &'a mut [u8],
    ) -> Result<Opened<'a>, SealError> {
        let result = self.open_inner(datagram, out);
        if result.is_err() {
            self.rejected = self.rejected.saturating_add(1);
        }
        result
    }

    fn open_inner<'a>(
        &mut self,
        datagram: &[u8],
        out: &'a mut [u8],
    ) -> Result<Opened<'a>, SealError> {
        if datagram.len() < SEALED_HEADER_BYTES + SEAL_TAG_BYTES {
            return Err(SealError::TooShort(datagram.len()));
        }
        if datagram[..SONDUIT_MAGIC.len()] != SONDUIT_MAGIC {
            return Err(SealError::BadMagic);
        }
        if datagram[4] != SEALED_VERSION {
            return Err(SealError::UnsupportedVersion(datagram[4]));
        }

        let declared = u16::from_le_bytes([datagram[18], datagram[19]]) as usize;
        let needed = Sealer::sealed_len(declared);
        if datagram.len() != needed {
            return Err(SealError::BadLength {
                expected: needed,
                actual: datagram.len(),
            });
        }
        if out.len() < declared {
            return Err(SealError::BadLength {
                expected: declared,
                actual: out.len(),
            });
        }

        let low = u16::from_le_bytes([datagram[6], datagram[7]]);
        let high = u32::from_le_bytes([datagram[20], datagram[21], datagram[22], datagram[23]]);
        let counter = (u64::from(high) << 16) | u64::from(low);

        let mut salt = [0_u8; SALT_BYTES];
        salt.copy_from_slice(&datagram[24..32]);
        // A stream this receiver has finished with is not allowed back.
        if self.stream.is_retired(&salt) {
            return Err(SealError::Replayed(counter));
        }

        // The window is per stream, so a fresh salt is not checked against the
        // counters of the stream before it.
        let known_stream = self.stream.cipher_for_salt(&salt).is_some();
        if known_stream && !self.stream.replay.admissible(counter) {
            return Err(SealError::Replayed(counter));
        }

        // A datagram naming an unknown salt is not allowed to replace the
        // cached key before it has proved itself. Otherwise a flood of random
        // salts would evict the real stream's key and force a derivation on
        // every genuine packet as well.
        let fresh = if known_stream {
            None
        } else {
            Some(cipher_for(&self.secret, &salt, true))
        };
        let cipher = match fresh.as_ref() {
            Some(cipher) => cipher,
            None => self
                .stream
                .cipher_for_salt(&salt)
                .ok_or(SealError::NotAuthentic)?,
        };

        let (header, body) = datagram.split_at(SEALED_HEADER_BYTES);
        let (ciphertext, tag) = body.split_at(declared);

        let plaintext = &mut out[..declared];
        plaintext.copy_from_slice(ciphertext);

        let mut tag_bytes = [0_u8; SEAL_TAG_BYTES];
        tag_bytes.copy_from_slice(tag);

        cipher
            .decrypt_in_place_detached(
                &nonce_for(counter),
                header,
                plaintext,
                &Tag::from(tag_bytes),
            )
            .map_err(|_| SealError::NotAuthentic)?;

        // Everything below here is data from the paired sender.
        let format = Format {
            sample_rate: Format::rate_from_marker(datagram[12])?,
            bit_depth: BitDepth::from_bits(datagram[13])?,
            channels: datagram[14],
            channel_mask: u16::from_le_bytes([datagram[16], datagram[17]]),
        };
        format.validate()?;

        if let Some(cipher) = fresh {
            self.stream.adopt(salt, cipher);
        }
        self.stream.replay.commit(counter);

        Ok(Opened {
            format,
            sequence: low,
            counter,
            timestamp_frames: u32::from_le_bytes([
                datagram[8],
                datagram[9],
                datagram[10],
                datagram[11],
            ]),
            flags: datagram[5],
            pcm: plaintext,
        })
    }
}

/// Seals feedback reports for one session.
///
/// The reverse direction is a control channel and can be forged too: a report
/// carries the loss figure, the buffer depth and the echo the sender measures
/// its round trip from, and a sender that believes a forged one shows numbers
/// that are not about the session it is running. It is four datagrams a
/// second, so covering it costs nothing measurable.
pub struct FeedbackSealer {
    cipher: ChaCha20Poly1305,
    salt: [u8; SALT_BYTES],
    counter: u64,
}

impl core::fmt::Debug for FeedbackSealer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FeedbackSealer")
            .field("counter", &self.counter)
            .finish_non_exhaustive()
    }
}

impl FeedbackSealer {
    /// A sealer for the reports of one session.
    #[must_use]
    pub fn new(secret: &SessionSecret, salt: [u8; SALT_BYTES]) -> Self {
        Self {
            cipher: cipher_for(secret, &salt, false),
            salt,
            counter: 0,
        }
    }

    /// Seal a report into `out`, which must hold [`SEALED_FEEDBACK_BYTES`].
    ///
    /// # Errors
    /// Returns [`SealError::BadLength`] when the buffer is too small.
    pub fn seal(&mut self, report: &Feedback, out: &mut [u8]) -> Result<usize, SealError> {
        if out.len() < SEALED_FEEDBACK_BYTES {
            return Err(SealError::BadLength {
                expected: SEALED_FEEDBACK_BYTES,
                actual: out.len(),
            });
        }

        let mut plain = [0_u8; FEEDBACK_BYTES];
        report
            .encode(&mut plain)
            .map_err(|_| SealError::BadLength {
                expected: FEEDBACK_BYTES,
                actual: plain.len(),
            })?;

        out[0..4].copy_from_slice(&crate::feedback::FEEDBACK_MAGIC);
        out[4] = SEALED_FEEDBACK_VERSION;
        out[5] = 0;
        out[6..14].copy_from_slice(&self.counter.to_le_bytes());
        out[14..22].copy_from_slice(&self.salt);

        let (header, body) =
            out[..SEALED_FEEDBACK_BYTES].split_at_mut(SEALED_FEEDBACK_HEADER_BYTES);
        let (ciphertext, tag_bytes) = body.split_at_mut(FEEDBACK_BYTES);
        ciphertext.copy_from_slice(&plain);

        let tag = self
            .cipher
            .encrypt_in_place_detached(&nonce_for(self.counter), header, ciphertext)
            .map_err(|_| SealError::NotAuthentic)?;
        tag_bytes.copy_from_slice(&tag);

        self.counter += 1;
        Ok(SEALED_FEEDBACK_BYTES)
    }
}

/// Opens feedback reports from one paired receiver.
pub struct FeedbackOpener {
    secret: SessionSecret,
    stream: StreamState,
    rejected: u64,
}

impl core::fmt::Debug for FeedbackOpener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FeedbackOpener")
            .field("rejected", &self.rejected)
            .finish_non_exhaustive()
    }
}

impl FeedbackOpener {
    /// An opener for a session, with nothing accepted yet.
    #[must_use]
    pub const fn new(secret: SessionSecret) -> Self {
        Self {
            secret,
            stream: StreamState::new(),
            rejected: 0,
        }
    }

    /// Reports refused since the session began.
    #[must_use]
    pub const fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Authenticate and decrypt a report.
    ///
    /// # Errors
    /// As [`Opener::open`]: a report that does not authenticate is an error
    /// and not a silently ignored datagram.
    pub fn open(&mut self, datagram: &[u8]) -> Result<Feedback, SealError> {
        let result = self.open_inner(datagram);
        if result.is_err() {
            self.rejected = self.rejected.saturating_add(1);
        }
        result
    }

    fn open_inner(&mut self, datagram: &[u8]) -> Result<Feedback, SealError> {
        if datagram.len() < SEALED_FEEDBACK_BYTES {
            return Err(SealError::TooShort(datagram.len()));
        }
        if datagram[0..4] != crate::feedback::FEEDBACK_MAGIC {
            return Err(SealError::BadMagic);
        }
        if datagram[4] != SEALED_FEEDBACK_VERSION {
            return Err(SealError::UnsupportedVersion(datagram[4]));
        }

        let mut counter_bytes = [0_u8; 8];
        counter_bytes.copy_from_slice(&datagram[6..14]);
        let counter = u64::from_le_bytes(counter_bytes);

        let mut salt = [0_u8; SALT_BYTES];
        salt.copy_from_slice(&datagram[14..22]);
        if self.stream.is_retired(&salt) {
            return Err(SealError::Replayed(counter));
        }

        let known_stream = self.stream.cipher_for_salt(&salt).is_some();
        if known_stream && !self.stream.replay.admissible(counter) {
            return Err(SealError::Replayed(counter));
        }

        let fresh = if known_stream {
            None
        } else {
            Some(cipher_for(&self.secret, &salt, false))
        };
        let cipher = match fresh.as_ref() {
            Some(cipher) => cipher,
            None => self
                .stream
                .cipher_for_salt(&salt)
                .ok_or(SealError::NotAuthentic)?,
        };

        let header = &datagram[..SEALED_FEEDBACK_HEADER_BYTES];
        let mut plain = [0_u8; FEEDBACK_BYTES];
        plain.copy_from_slice(
            &datagram[SEALED_FEEDBACK_HEADER_BYTES..SEALED_FEEDBACK_HEADER_BYTES + FEEDBACK_BYTES],
        );

        let mut tag_bytes = [0_u8; SEAL_TAG_BYTES];
        tag_bytes.copy_from_slice(&datagram[SEALED_FEEDBACK_HEADER_BYTES + FEEDBACK_BYTES..]);

        cipher
            .decrypt_in_place_detached(
                &nonce_for(counter),
                header,
                &mut plain,
                &Tag::from(tag_bytes),
            )
            .map_err(|_| SealError::NotAuthentic)?;

        let report = Feedback::decode(&plain).ok_or(SealError::NotAuthentic)?;

        if let Some(cipher) = fresh {
            self.stream.adopt(salt, cipher);
        }
        self.stream.replay.commit(counter);

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::{PairingCode, NONCE_BYTES};
    use crate::session::{KeyExchange, SEED_BYTES};
    use sonduit_core::format::PCM_PAYLOAD_BYTES;

    const SALT: [u8; SALT_BYTES] = [0xA5; SALT_BYTES];

    fn secret(seed: u8) -> SessionSecret {
        let nonce = [0x5A; NONCE_BYTES];
        let code = PairingCode::parse("482913").unwrap();
        let desktop = KeyExchange::from_seed([seed; SEED_BYTES]);
        let phone = KeyExchange::from_seed([seed.wrapping_add(1); SEED_BYTES]);
        let (pa, pb) = (desktop.public_key(), phone.public_key());
        desktop.agree(&pb, &nonce, &code, &pa, &pb).unwrap()
    }

    fn payload() -> Vec<u8> {
        (0..PCM_PAYLOAD_BYTES)
            .map(|index| (index % 251) as u8)
            .collect()
    }

    fn seal_one(sealer: &mut Sealer, pcm: &[u8]) -> Vec<u8> {
        let mut datagram = vec![0_u8; Sealer::sealed_len(pcm.len())];
        sealer
            .seal(&Format::stereo_48k(), 0, 0, pcm, &mut datagram)
            .unwrap();
        datagram
    }

    #[test]
    fn a_sealed_packet_round_trips_to_the_same_audio() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());

        let pcm = payload();
        let mut datagram = vec![0_u8; Sealer::sealed_len(pcm.len())];
        sealer
            .seal(&Format::stereo_48k(), 4242, 0, &pcm, &mut datagram)
            .unwrap();

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        let opened = opener.open(&datagram, &mut out).unwrap();

        assert_eq!(opened.pcm, &pcm[..]);
        assert_eq!(opened.format, Format::stereo_48k());
        assert_eq!(opened.timestamp_frames, 4242);
        assert_eq!(opened.sequence, 0);
        assert_eq!(opened.counter, 0);
    }

    #[test]
    fn the_ciphertext_is_not_the_plaintext() {
        // The failure this whole module exists to prevent would still pass a
        // round-trip test if the cipher were a no-op.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let pcm = payload();
        let datagram = seal_one(&mut sealer, &pcm);

        let body = &datagram[SEALED_HEADER_BYTES..SEALED_HEADER_BYTES + pcm.len()];
        assert_ne!(body, &pcm[..], "the payload went out in the clear");
    }

    #[test]
    fn the_datagram_is_the_documented_size() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let datagram = seal_one(&mut sealer, &payload());
        assert_eq!(datagram.len(), 32 + PCM_PAYLOAD_BYTES + 16);
        assert!(datagram.len() < crate::MAX_DATAGRAM_BYTES);
    }

    #[test]
    fn a_tampered_packet_is_refused_rather_than_played() {
        let shared = secret(1);
        let pcm = payload();

        // Every byte of the datagram, header and body alike, since the header
        // is the associated data and must be covered too.
        for at in [0_usize, 5, 6, 8, 12, 18, 20, 24, 32, 700, 1183] {
            let mut sealer = Sealer::new(&shared, SALT);
            let mut opener = Opener::new(shared.clone());
            let mut datagram = seal_one(&mut sealer, &pcm);
            datagram[at] ^= 0x01;

            let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
            assert!(
                opener.open(&datagram, &mut out).is_err(),
                "byte {at} could be rewritten"
            );
        }
    }

    #[test]
    fn flipping_the_link_flag_is_refused() {
        // The receiver sizes its jitter buffer from this bit. Under version 1
        // anyone could set it; here it is inside the associated data.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());

        let mut datagram = seal_one(&mut sealer, &payload());
        datagram[5] ^= sonduit_core::packet::FLAG_WIRED_LINK;

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::NotAuthentic)
        ));
    }

    #[test]
    fn a_packet_from_a_peer_with_the_wrong_key_is_refused() {
        // The eavesdropping case in reverse: somebody on the network who never
        // paired sends a well-formed packet.
        let mine = secret(1);
        let theirs = secret(40);

        let mut sealer = Sealer::new(&theirs, SALT);
        let mut opener = Opener::new(mine);
        let datagram = seal_one(&mut sealer, &payload());

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::NotAuthentic)
        ));
        assert_eq!(opener.rejected(), 1);
    }

    #[test]
    fn a_replayed_packet_is_refused_the_second_time() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let datagram = seal_one(&mut sealer, &payload());

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        assert!(opener.open(&datagram, &mut out).is_ok());
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::Replayed(0))
        ));
    }

    #[test]
    fn a_packet_older_than_the_window_is_refused() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let pcm = payload();

        let first = seal_one(&mut sealer, &pcm);
        let mut latest = Vec::new();
        for _ in 0..(REPLAY_WINDOW_PACKETS + 8) {
            latest = seal_one(&mut sealer, &pcm);
        }

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        assert!(opener.open(&latest, &mut out).is_ok());
        assert!(matches!(
            opener.open(&first, &mut out),
            Err(SealError::Replayed(0))
        ));
    }

    #[test]
    fn reordering_inside_the_window_is_accepted_in_any_order() {
        // UDP reorders by design. A construction that only accepted packets in
        // order would turn every reordering into a dropout.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let pcm = payload();

        let datagrams: Vec<Vec<u8>> = (0..8).map(|_| seal_one(&mut sealer, &pcm)).collect();
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];

        for index in [7_usize, 3, 5, 0, 6, 1, 4, 2] {
            let opened = opener
                .open(&datagrams[index], &mut out)
                .unwrap_or_else(|error| panic!("packet {index} refused: {error}"));
            assert_eq!(opened.counter, index as u64);
        }
    }

    #[test]
    fn a_gap_in_the_stream_does_not_stop_the_packets_after_it() {
        // Loss is the normal case on this transport. Only the lost packets may
        // be lost; the ones behind them must still open.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let pcm = payload();

        let datagrams: Vec<Vec<u8>> = (0..10).map(|_| seal_one(&mut sealer, &pcm)).collect();
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];

        for (index, datagram) in datagrams.iter().enumerate() {
            if (3..7).contains(&index) {
                continue;
            }
            assert!(opener.open(datagram, &mut out).is_ok(), "packet {index}");
        }
    }

    #[test]
    fn the_counter_survives_a_sequence_number_wrap() {
        // This is the whole reason the counter is not the sequence number: at
        // six milliseconds a packet the u16 wraps every 393 seconds, and a
        // nonce that repeated there would give away the key.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let pcm = vec![7_u8; 32];

        let mut first_wrapped = None;
        for _ in 0..=(u32::from(u16::MAX) + 2) {
            let datagram = seal_one(&mut sealer, &pcm);
            let sequence = u16::from_le_bytes([datagram[6], datagram[7]]);
            if sequence == 0 && first_wrapped.is_none() && sealer.counter() > 1 {
                first_wrapped = Some(datagram.clone());
            }
            let mut out = vec![0_u8; 32];
            let opened = opener.open(&datagram, &mut out).unwrap();
            assert_eq!(opened.sequence, sequence);
        }

        let wrapped = first_wrapped.expect("the sequence never wrapped");
        assert_eq!(u16::from_le_bytes([wrapped[6], wrapped[7]]), 0);
        // Sequence zero, counter 65536: the high half of the counter is what
        // keeps the two nonces apart.
        let high = u32::from_le_bytes([wrapped[20], wrapped[21], wrapped[22], wrapped[23]]);
        assert_eq!(high, 1);
    }

    #[test]
    fn every_packet_of_a_stream_uses_a_different_nonce() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let pcm = vec![0_u8; 16];

        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let counter = sealer.counter();
            let _ = seal_one(&mut sealer, &pcm);
            assert!(seen.insert(counter), "counter {counter} used twice");
        }
    }

    #[test]
    fn a_restarted_stream_uses_a_new_key_rather_than_a_new_counter() {
        // The counter goes back to zero on every stream. What must not go back
        // is the key, or the second stream would reuse the first's nonces.
        let shared = secret(1);
        let pcm = payload();

        let mut first = Sealer::new(&shared, [1; SALT_BYTES]);
        let mut second = Sealer::new(&shared, [2; SALT_BYTES]);
        let one = seal_one(&mut first, &pcm);
        let two = seal_one(&mut second, &pcm);

        assert_eq!(one[6..8], two[6..8], "both start at sequence zero");
        assert_ne!(
            one[SEALED_HEADER_BYTES..],
            two[SEALED_HEADER_BYTES..],
            "the same plaintext at the same counter produced the same ciphertext"
        );
    }

    #[test]
    fn a_receiver_follows_the_sender_into_a_new_stream() {
        let shared = secret(1);
        let mut opener = Opener::new(shared.clone());
        let pcm = payload();
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];

        let mut first = Sealer::new(&shared, [1; SALT_BYTES]);
        for _ in 0..4 {
            let datagram = seal_one(&mut first, &pcm);
            assert!(opener.open(&datagram, &mut out).is_ok());
        }

        // A new stream starts its counter at zero again. Without resetting the
        // window on the new salt this would look like a replay.
        let mut second = Sealer::new(&shared, [2; SALT_BYTES]);
        let datagram = seal_one(&mut second, &pcm);
        assert!(opener.open(&datagram, &mut out).is_ok());
    }

    #[test]
    fn an_unknown_salt_does_not_evict_the_working_key() {
        // Otherwise a flood of random salts would force a key derivation on
        // every genuine packet as well as every forged one.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let pcm = payload();
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];

        assert!(opener.open(&seal_one(&mut sealer, &pcm), &mut out).is_ok());

        let mut junk = seal_one(&mut sealer, &pcm);
        junk[24..32].copy_from_slice(&[0xFF; SALT_BYTES]);
        assert!(opener.open(&junk, &mut out).is_err());

        assert!(
            opener.open(&seal_one(&mut sealer, &pcm), &mut out).is_ok(),
            "the real stream stopped working after a forged salt"
        );
    }

    #[test]
    fn a_cleartext_packet_is_not_opened_as_a_sealed_one() {
        let shared = secret(1);
        let mut opener = Opener::new(shared);

        let pcm = payload();
        let packet = sonduit_core::packet::SonduitPacket {
            format: Format::stereo_48k(),
            sequence: 0,
            timestamp_frames: 0,
            flags: 0,
            pcm: &pcm,
        };
        let mut datagram = vec![0_u8; sonduit_core::packet::SonduitPacket::encoded_len(pcm.len())];
        packet.encode(&mut datagram).unwrap();

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::UnsupportedVersion(1))
        ));
        assert!(!Opener::is_sealed(&datagram));
    }

    #[test]
    fn a_truncated_datagram_is_refused_rather_than_panicking() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let datagram = seal_one(&mut sealer, &payload());

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        for length in 0..datagram.len() {
            assert!(
                opener.open(&datagram[..length], &mut out).is_err(),
                "accepted a datagram truncated to {length} bytes"
            );
        }
    }

    #[test]
    fn a_lying_length_field_is_refused() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let mut datagram = seal_one(&mut sealer, &payload());
        datagram[18..20].copy_from_slice(&((PCM_PAYLOAD_BYTES + 1) as u16).to_le_bytes());

        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES * 2];
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::BadLength { .. })
        ));
    }

    #[test]
    fn an_output_buffer_that_is_too_small_is_reported_not_overrun() {
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let mut opener = Opener::new(shared.clone());
        let datagram = seal_one(&mut sealer, &payload());

        let mut out = vec![0_u8; 16];
        assert!(matches!(
            opener.open(&datagram, &mut out),
            Err(SealError::BadLength { .. })
        ));
    }

    #[test]
    fn a_feedback_report_round_trips_and_a_forged_one_does_not() {
        let shared = secret(1);
        let report = Feedback {
            echo: 12_345,
            hold_ms: 7,
            accepted: 900,
            lost: 3,
            depth_tenths_ms: 210,
            queue_tenths_ms: Some(44),
            playing: true,
        };

        let mut sealer = FeedbackSealer::new(&shared, SALT);
        let mut opener = FeedbackOpener::new(shared.clone());
        let mut datagram = vec![0_u8; SEALED_FEEDBACK_BYTES];
        assert_eq!(
            sealer.seal(&report, &mut datagram).unwrap(),
            SEALED_FEEDBACK_BYTES
        );

        let mut second = vec![0_u8; SEALED_FEEDBACK_BYTES];
        sealer.seal(&report, &mut second).unwrap();

        assert_eq!(opener.open(&datagram).unwrap(), report);

        // The loss figure drives what the user is shown; rewriting it must not
        // work, and neither must sending the same report twice.
        let mut tampered = second.clone();
        tampered[SEALED_FEEDBACK_HEADER_BYTES + 20] ^= 0xFF;
        assert!(matches!(
            opener.open(&tampered),
            Err(SealError::NotAuthentic)
        ));
        assert!(matches!(
            opener.open(&datagram),
            Err(SealError::Replayed(0))
        ));
        // The tampering must not have burned the counter: the genuine report
        // at that counter still has to be accepted.
        assert_eq!(opener.open(&second).unwrap(), report);
    }

    #[test]
    fn a_cleartext_feedback_report_is_not_opened() {
        let shared = secret(1);
        let mut opener = FeedbackOpener::new(shared);
        let report = Feedback {
            echo: 1,
            hold_ms: 0,
            accepted: 0,
            lost: 0,
            depth_tenths_ms: 0,
            queue_tenths_ms: None,
            playing: false,
        };
        let mut plain = vec![0_u8; SEALED_FEEDBACK_BYTES];
        report.encode(&mut plain).unwrap();

        assert!(matches!(
            opener.open(&plain),
            Err(SealError::UnsupportedVersion(1))
        ));
    }

    #[test]
    fn an_audio_key_does_not_open_a_feedback_report() {
        // Separate keys per direction, so a report cannot be replayed into the
        // audio path or the other way round.
        let shared = secret(1);
        let mut sealer = Sealer::new(&shared, SALT);
        let datagram = seal_one(&mut sealer, &payload());

        let mut feedback = FeedbackOpener::new(shared.clone());
        assert!(feedback.open(&datagram).is_err());
    }

    #[test]
    fn a_finished_stream_cannot_be_replayed_after_the_sender_restarts() {
        // Every packet of an old stream authenticates, because every packet of
        // it was genuine. What makes it an attack is that a new salt resets
        // the replay window, so without retiring the old salt a recording of
        // the stream before last could be played back into this one.
        let shared = secret(1);
        let pcm = payload();
        let mut out = vec![0_u8; PCM_PAYLOAD_BYTES];
        let mut opener = Opener::new(shared.clone());

        let mut first = Sealer::new(&shared, [1; SALT_BYTES]);
        let recorded: Vec<Vec<u8>> = (0..4).map(|_| seal_one(&mut first, &pcm)).collect();
        for datagram in &recorded {
            assert!(opener.open(datagram, &mut out).is_ok());
        }

        let mut second = Sealer::new(&shared, [2; SALT_BYTES]);
        assert!(opener.open(&seal_one(&mut second, &pcm), &mut out).is_ok());

        for datagram in &recorded {
            assert!(
                matches!(opener.open(datagram, &mut out), Err(SealError::Replayed(_))),
                "a packet from the finished stream was accepted"
            );
        }
        assert!(opener.open(&seal_one(&mut second, &pcm), &mut out).is_ok());
    }

    #[test]
    fn only_the_last_few_streams_are_remembered_and_that_is_bounded() {
        // The list of retired salts must not grow with the session. Eight is
        // far more restarts than a session sees, and the ones that fall off
        // the end are streams no recording is still relevant to.
        let shared = secret(1);
        let pcm = vec![5_u8; 64];
        let mut out = vec![0_u8; 64];
        let mut opener = Opener::new(shared.clone());

        let mut oldest = Sealer::new(&shared, [0; SALT_BYTES]);
        let recorded = seal_one(&mut oldest, &pcm);
        assert!(opener.open(&recorded, &mut out).is_ok());

        for salt in 1..=(RETIRED_SALTS as u8 + 1) {
            let mut sealer = Sealer::new(&shared, [salt; SALT_BYTES]);
            assert!(opener.open(&seal_one(&mut sealer, &pcm), &mut out).is_ok());
        }

        // The oldest salt has fallen off the list, so its packets are opened
        // again. That is the accepted cost of a fixed-size memory, and it is
        // nine stream restarts away.
        assert!(opener.open(&recorded, &mut out).is_ok());
    }

    #[test]
    fn a_finished_feedback_stream_cannot_be_replayed_either() {
        let shared = secret(1);
        let report = Feedback {
            echo: 5,
            hold_ms: 1,
            accepted: 10,
            lost: 0,
            depth_tenths_ms: 100,
            queue_tenths_ms: None,
            playing: true,
        };
        let mut opener = FeedbackOpener::new(shared.clone());

        let mut first = FeedbackSealer::new(&shared, [1; SALT_BYTES]);
        let mut recorded = vec![0_u8; SEALED_FEEDBACK_BYTES];
        first.seal(&report, &mut recorded).unwrap();
        assert!(opener.open(&recorded).is_ok());

        let mut second = FeedbackSealer::new(&shared, [2; SALT_BYTES]);
        let mut fresh = vec![0_u8; SEALED_FEEDBACK_BYTES];
        second.seal(&report, &mut fresh).unwrap();
        assert!(opener.open(&fresh).is_ok());

        assert!(matches!(
            opener.open(&recorded),
            Err(SealError::Replayed(_))
        ));
    }

    #[test]
    fn the_replay_window_shifts_without_losing_recent_packets() {
        let mut window = ReplayWindow::default();
        window.commit(0);
        window.commit(1);
        window.commit(70);

        assert!(!window.admissible(0));
        assert!(!window.admissible(1));
        assert!(!window.admissible(70));
        assert!(window.admissible(2));
        assert!(window.admissible(69));
        assert!(window.admissible(71));
    }

    #[test]
    fn a_jump_past_the_window_clears_it_rather_than_shifting_wrongly() {
        let mut window = ReplayWindow::default();
        window.commit(0);
        window.commit(10_000);

        assert!(!window.admissible(10_000));
        assert!(window.admissible(9_999));
        assert!(!window.admissible(0), "far older than the window");
    }
}
