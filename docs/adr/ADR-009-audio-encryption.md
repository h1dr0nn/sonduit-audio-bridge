# ADR-009: Encrypt the audio, keyed by a Diffie-Hellman the pairing code authenticates

- Status: accepted
- Date: 2026-08-28
- Amends: [ADR-006](ADR-006-discovery.md), whose "deliberately excluded for
  now: authentication and pairing" consequence is now fully answered, and
  whose discovery protocol gains two messages and a version
- Amends: [ADR-005](ADR-005-wire-protocol.md), which reserved a version field
  and a flags byte for exactly this; this is the first use of the version

## Context

Pairing landed and stopped an unpaired device being *selected*. It did nothing
about the wire. The roadmap's "the audio itself is not encrypted" entry, the
README and
`crates/sonduit-transport/src/pairing.rs` all said the same thing in their own
words: **the PCM is in the clear.** Anyone on the same Wi-Fi can reconstruct
what the machine is playing, and anyone can inject a datagram into a running
session, which the receiver will play, because nothing on the audio path proves
who sent it.

The second half is not theoretical. Commit `c62cdd0` confirms the link-kind
flag over five consecutive packets, and its reasoning is written down: nothing
authenticates this wire, so one injected packet must not be able to retune the
jitter buffer.

## The problem the pairing code creates

The obvious design is to derive a key from the six-digit code with a KDF. It
does not work, and it is worth being precise about why.

Six digits is 10^6 possibilities: **19.93 bits.** The code never travels on the
wire, only an HMAC-SHA256 keyed by it over a known nonce and a mostly known
body. That HMAC is an **offline verifier**. An attacker who captures one
pairing exchange -- one probe and one announcement, on a network they were
already sitting on -- can try all 10^6 codes against it locally. On a laptop
that is under a second; a KDF in front of it changes the number to seconds, not
to years, because the search space and not the hash is the limit.

So:

| The code can | The code cannot |
| --- | --- |
| Stop an unpaired device being selected, against an attacker who was not listening when the pairing happened | Stay secret from anyone who captured the pairing exchange |
| Authenticate one live exchange, at odds of 1 in 10^6 per online attempt | Be key material for anything, under any KDF |
| Bind a transcript so a swapped public key is detected | Protect a recording made before it was cracked |

**A session key derived from the code would be a session key an eavesdropper
recovers in under a second, and audio "encrypted" under it would be readable by
exactly the attacker the encryption is for.** That is the outcome this ADR
exists to avoid.

## Decision

### 1. The code authenticates a Diffie-Hellman; it is not the key

Pairing gains two datagrams, `KeyOffer` and `KeyAccept`, carrying an ephemeral
**X25519** public key each and tagged with the same HMAC-SHA256 keyed by the
same code that already tags the announcement:

```text
  desktop                                             phone
    |  probe carrying nonce N, or the same N in a QR    |
    | ------------------------------------------------> |
    |  announce: name, port, HMAC(code; N, body)        |
    | <------------------------------------------------ |
    |  key offer:  PA, HMAC(code; N, 3, PA)             |
    | ------------------------------------------------> |
    |  key accept: PB, HMAC(code; N, 4, PA, PB)         |
    | <------------------------------------------------ |
```

Both ends then hold `X25519(a, PB) == X25519(b, PA)` and derive

```text
master = HKDF-SHA256(salt = N, ikm = shared || code, info = "sonduit-session-v1" || PA || PB)
```

What this changes, exactly:

- **An eavesdropper still cracks the code, and still learns nothing.** X25519
  is not weakened by knowing the code, so a recording of the pairing *and* the
  whole session stays unreadable. This is the property that matters, and it is
  the one a code-derived key does not have.
- **An active attacker must be present during the pairing window** and must
  forge a tag over its own public key: one online guess in 10^6, against a code
  that is regenerated for the next window. Twenty bits online and single-shot
  is the level six-digit numeric pairing has anywhere it is used; Bluetooth
  numeric comparison makes the same trade.
- The transcript (`N`, `PA`, `PB`, the code) is bound into the derivation, so a
  key swapped in flight leaves two ends that cannot talk rather than one
  session on an attacker's terms.
- A peer public key that is one of the small-order points is refused. Accepting
  one would force the shared secret to zero for everybody.

Two extra datagrams rather than fields inside the probe and the announcement,
because **the QR pairing path has no probe on the wire at all**: the invite is
delivered optically and the phone answers it directly (`docs/protocol.md`
section 7.4), so a public key carried in the probe would never reach it. One
handshake that works on both paths is worth one round trip that happens once
per pairing, and it leaves the `SDQ1` invite grammar and
`discovery::encode_announce` untouched.

### 1a. The offer is sent more than once, so answering it is idempotent

Amended after the exchange was measured on hardware. It is part of the
decision, not a note on it: without it the design above agrees no key at all.

Each of the four datagrams is repeated for loss tolerance -- a single datagram
after an idle period is regularly dropped on Wi-Fi, and the probe already went
three times for that reason. The key offer goes three times too. **The
responder therefore sees one offer three times and must answer all three
identically**, because the initiator keeps the first accept that reaches it and
has no way to ask which key the responder kept.

The first implementation drew a fresh seed per offer and adopted the last. Both
ends reported a completed pairing and **every packet of every encrypted session
was refused**: 18186 sent, 18428 refusals counted at the receiver, nothing
played. Isolated on hardware by varying nothing but the offer count -- one
offer played all 200 packets, three offers refused all 200.

So the responder remembers the offers it has answered:

- **The memory is keyed on the exchange**, meaning the initiator's public key
  under the nonce it is tagged against. Both are covered by the offer's tag, so
  neither can be altered by anyone who does not hold the code, and a genuinely
  new exchange -- a fresh seed, a fresh pairing window -- differs in one or the
  other and draws a new key pair as before.
- **A repeat is answered with the recorded accept and adopts nothing.** The
  secret was handed over on the first answer; re-adopting it would let a
  replayed old offer displace a pairing made since, which the previous
  behaviour of minting a fresh key could also do.
- **Only public bytes are remembered**, four exchanges deep. No key material is
  retained beyond the one secret the receiver already holds, so nothing here
  changes what a compromise of the process yields.

**An attacker gains nothing from an answer that repeats.** Replaying a captured
offer returns the accept that was already on the wire beside it, and both
halves are public keys. It buys no extra guess at the code, because the memory
is consulted only after the offer's tag has verified: a forged offer still
costs one online guess in 10^6 and a wrong one is still met with silence. The
"one online guess in a million" property of the section above is unchanged.

The initiator does not simply trust this. It keeps listening for 50 ms after
the first accept verifies and **refuses the pairing if two accepts differ**,
which turns a peer built the old way into a message at the pairing rather than
a session that reports itself encrypted and plays nothing. Nobody outside the
pairing can provoke that refusal: an accept only counts once its tag verifies
under the six-digit code.

Two alternatives were weighed and rejected. **Tolerating several accepts and
adopting the one that matches** fails because the initiator cannot tell which
one that is -- an accept carries a public key, not a proof of what the peer
kept -- so it would need another round trip. **One offer with a retry only on
timeout** narrows the window without closing it: an accept that is merely slow
rather than lost still leaves the initiator holding the answer to the first
offer and the responder holding the key it made for the second, and it is
slower to recover from the loss the retransmission exists for.

### 2. ChaCha20-Poly1305, in place, per packet

`chacha20poly1305`, RustCrypto, `default-features = false`.

- **Not AES-GCM.** Half of this product runs on ARM and `armeabi-v7a` has no
  AES instructions at all, so software AES there is both slower and harder to
  keep constant-time. ChaCha20 is the reason TLS carries it for mobile.
- **Not a hand-rolled anything.** Same argument the pairing HMAC already made.
- `AeadInPlace::encrypt_in_place_detached` takes `&mut [u8]` and returns the
  tag, so the payload is copied into the caller's datagram buffer once and
  encrypted where it lies. **Nothing on either path allocates per packet**,
  which is the realtime contract in CONTRIBUTING.

### 3. The nonce is the packet counter, and it does not wrap

This is the part a `u16` sequence number makes interesting. At 6 ms a packet it
wraps every **393 seconds**, which is a normal event in any real session, and a
nonce that repeated there would hand over the keystream and Poly1305's own key.

So the nonce is **not** the sequence number. The sender owns a 48-bit packet
counter:

- its low 16 bits *are* the sequence number, in header bytes 6..8, so the
  jitter buffer, the loss counter and the round-trip estimator all keep working
  unchanged;
- its high 32 bits ride in header bytes 20..24.

`nonce = counter as little-endian u64, zero-padded to 12 bytes`. At 6 ms a
packet, 2^48 packets is **53 million years**. There is no wrap to handle and no
sequence reconstruction to get wrong, so an injected packet cannot push a
receiver's idea of the counter out of step: the counter is read from the
datagram, and a datagram whose counter is wrong simply fails to authenticate.

The other half of nonce safety is the counter restarting at zero when a stream
does. That is answered by making the **key** fresh instead: eight random bytes
of stream salt in header bytes 24..32 go into

```text
audio_key = HKDF-SHA256(salt = stream_salt, ikm = master, info = "sonduit-audio-v1")
```

so the second stream under one pairing never runs under the first stream's key.
Carrying the salt in every datagram, rather than negotiating it, also means a
receiver that joins or restarts mid-stream is in step on the first packet it
sees. There is no resynchronisation path, because there is nothing to
resynchronise.

### 4. The header is authenticated, not encrypted

The whole 32-byte header is the AEAD's associated data. This is SRTP's
arrangement and it is deliberate: the receiver has to route a datagram, pick a
key by salt and reject a replay before it can afford to authenticate anything,
and the jitter buffer wants the sequence number whether or not the packet turns
out to be good.

What leaks is the sample rate, the channel count and a frame counter. What is
covered is everything the receiver acts on -- including `FLAG_WIRED_LINK`,
which sizes the jitter buffer. **`c62cdd0`'s five-packet confirmation exists
because that bit was unauthenticated. Under this format only the paired sender
can set it**, so that rule can be simplified whenever somebody wants to.

### 5. The cipher layer keeps its own replay window

`Opener` holds a 256-packet sliding window, the RFC 4303 construction: a
highest-accepted counter and a bitmap below it. 256 packets is about 1.5
seconds, comfortably wider than any jitter buffer this project configures, so
it never rejects a packet the buffer could have used. Nothing is recorded until
a packet has authenticated, so forged counters cannot push the window forward
and lock the real sender out.

**Is the jitter buffer's own window not enough?** For a running session it
usually is: it rejects anything behind the playback point and anything already
held. But it is not the same guarantee. It is disabled while the buffer is
still filling, which is exactly when a session is most attackable; its width is
tuned for latency and is expected to be retuned; and it lives in another crate
at another layer. A security property that holds only because of another
component's current tuning is a property that breaks silently the day that
component is tuned. The window here costs four `u64`s and two shifts.

The window has to reset when the stream salt changes, because the new stream's
counter starts again at zero. That reset is itself a hole, and it is the
subtlest thing in this design: **every packet of an old stream authenticates,
because every packet of it was genuine.** Somebody who recorded the stream
before last could replay it after the sender restarts and it would be accepted
as the current one. So the last eight retired salts are remembered and refused.
Sixty-four bytes, fixed size, and a genuine salt never repeats because it is
eight random bytes chosen per stream.

### 6. Encryption is a version bump, and the refusal happens at pairing

`SONDUIT_VERSION = 1` is cleartext; **sealed packets are version 2.**

**Not a flag bit.** Every receiver already built ignores unknown flags, so a
flag would have it decode the ciphertext as PCM and play it: a full-scale noise
burst into somebody's headphones. A version it does not know is refused by
`SonduitPacket::decode` before a byte of payload is looked at, and that refusal
is already tested.

But a refused audio packet still *looks* like a broken link, and CONTRIBUTING
is right that this is the worst outcome. So the real compatibility check is one
layer earlier and one exchange sooner: **`DISCOVERY_VERSION` goes from 2 to
3.** Two peers settle whether they can encrypt before any audio flows, in an
exchange that already fails loudly.

| Meeting | What happens |
| --- | --- |
| New desktop, old phone | The phone does not recognise a version 3 probe and does not answer. The desktop reports no device found. `discovery::foreign_version` says the datagram was version 2, so the message can be "that phone is running an older Sonduit" rather than silence. |
| Old desktop, new phone | The phone does not answer a version 2 probe. Same outcome, same message available. |
| New desktop, new phone | Four datagrams, a shared secret, encrypted audio. |
| Sealed packet reaching an old receiver | Cannot happen -- it never paired -- but if it did: `Err(UnsupportedVersion(2))`, refused, not played. |
| Cleartext packet reaching a keyed receiver | Refused, `Err(UnsupportedVersion(1))`. **This is the downgrade defence and it is not optional:** a keyed receiver that still accepted version 1 would let an attacker simply send version 1, and the encryption would be decoration. |

`SONDUIT_VERSION_SEALED` is named in `sonduit-core::packet` so the two crates
cannot drift into using the same byte for different things.

### 7. The feedback datagram is covered too

It is a control channel and it can be forged. A report carries the loss figure,
the buffer depth and the echo the sender measures its round trip from, so a
forged one makes the sender show numbers that are not about the session it is
running -- and `FEEDBACK_TIMEOUT_MS` means a forged one can also keep a dead
session looking alive. It is four datagrams a second. Covering it costs nothing
measurable and leaving it would have been the only unauthenticated thing left.

Sealed reports are `FEEDBACK_MAGIC`, version 2, an 8-byte counter, the stream
salt, then the sealed version 1 body and its tag: 72 bytes. The key is derived
with a **different label** (`"sonduit-feedback-v1"`), so a report can never be
replayed into the audio path or the reverse. The version 1 encoding is
untouched, and the reserved-byte trick that `2ccccc1` used to add the queue
depth still works inside the sealed body.

### 8. Cost

Measured by `cargo run --release --example seal_cost -p sonduit-transport` on
an i5-12400, 65 536 packets per figure, 1152-byte payloads:

| Stage | Per packet | Of the 6 ms a packet lasts |
| --- | --- | --- |
| Version 1 encode, for scale | 0.014 us | 0.0002% |
| **Seal**, on the capture thread | **1.94 - 2.02 us** | **0.033%** |
| **Open**, on the receive thread | **2.09 - 2.28 us** | **0.035%** |

Five runs, spread as shown. The receiver pays slightly more than the sender
because it copies the ciphertext into its plaintext buffer before decrypting
in place, which the sender folds into the copy it was making anyway.

Forcing the portable software backends (`chacha20_force_soft`,
`poly1305_force_soft`), which is the closest proxy this machine has for an ARM
build with no SIMD path, gives 2.21 us and 2.33 us -- so the figure is not
resting on AVX2. A mid-range phone core should land within a small multiple of
that, and even ten times the measured cost would be 0.35% of the budget.

`docs/latency-budget.md` owes a line for this: **+0.002 ms on the sender's
packetisation stage and +0.002 ms on the receiver's decode stage.** That is
below the resolution of every other row in that table, but CONTRIBUTING asks
for the stage and the number and this is both.

## Dependencies added

All four are pure Rust, all four resolve to the `digest 0.10` /
`generic-array 0.14` generation the existing `hmac` and `sha2` already pull, so
none of them duplicates a version. All are in `sonduit-transport`;
**`sonduit-core` gains nothing and keeps `forbid(unsafe_code)`.**

| Crate | Licence | Why |
| --- | --- | --- |
| `chacha20poly1305` 0.10 | MIT OR Apache-2.0 | The AEAD. `default-features = false` drops `alloc` and `getrandom`, and with them `libc` from the Android builds. |
| `hkdf` 0.12 | MIT OR Apache-2.0 | Per-stream, per-direction key derivation. A thin wrapper over the `hmac` already here. |
| `x25519-dalek` 2 | BSD-3-Clause | The key agreement. `default-features = false` with `static_secrets`, which is what allows a key pair to be built from a caller-supplied seed -- this crate does no I/O and cannot read the system entropy source itself. |
| `zeroize` 1 | MIT OR Apache-2.0 | Key material wiped on drop. Already in the tree under `chacha20poly1305`; naming it directly is what lets the session types derive `ZeroizeOnDrop`. |
| `getrandom` 0.3 | MIT OR Apache-2.0 | The platform's cryptographic random source, added when the handshake was wired up. An X25519 private key's whole strength is its seed, and the two ad-hoc generators already in this tree seed six-digit pairing codes, whose strength is twenty bits whatever they are seeded from. It is the one crate here that puts `libc` back on the Android build; reading the system entropy source is what `libc` is for, and the alternative was two hand-written copies in the two application crates, which is how a fallback to a clock reading gets added later by somebody in a hurry. |

Every licence in the resulting subtree is on the `tools/deny.toml` allowlist,
verified with `cargo deny --config tools/deny.toml --workspace check licenses
bans`.

`curve25519-dalek` contains `unsafe` in its SIMD backend, which is not built on
stable and is not what these targets resolve to; the serial `fiat-crypto`
backend is safe Rust. This is a dependency and not our crate, so the
`forbid(unsafe_code)` in `sonduit-core` and `sonduit-transport` is unaffected.

## Consequences

- **48 bytes of overhead per packet** rather than 20: a 32-byte header and a
  16-byte tag, so 1200 bytes on the wire against 1172. That is 4.2% overhead
  against 1.7%, and 37 kbit/s on a 1.536 Mbit/s stream. Still far below any
  MTU, so datagrams still never fragment.
- **The wire is authenticated, so rules that exist because it was not can be
  simplified.** `c62cdd0`'s five-packet link-flag confirmation is the one this
  ADR knows about.
- **Scream compatibility is unencrypted, necessarily.** Scream's five-byte
  header has no version field and nowhere to put a tag, so there is no sealed
  Scream and `Packetizer::sealed` does not offer one. A user who chooses the
  Scream wire is choosing an unmodified third-party driver as their sender, and
  that driver has no key. This must be said in the UI rather than left to be
  discovered.
- **A pairing is now worth keeping.** Before this, re-pairing cost nothing.
  Now the master secret is what a session is keyed from, so anything that
  discards it discards the ability to talk to that device.
- **This is live in both ends as of the commit that follows this ADR.** The
  desktop runs the handshake on both pairing paths and builds a
  `Packetizer::sealed`; the phone holds the master secret and routes sealed
  packets to an `Opener`. They landed together, because a keyed receiver
  refuses cleartext by design and one end alone would be a bridge that plays
  nothing.

  `DISCOVERY_VERSION` is 3, and it is worth still stating precisely what that
  means: **version 3 means "this build's discovery protocol carries the key
  agreement messages"**. It is now also true that a build speaking it
  encrypts, but the two remain different claims and only the first is what the
  version byte can settle.

  Nothing has shipped (`docs/roadmap.md`: no `release-v*` tag exists), so the
  version bump strands no installed build.
- **A session that cannot be encrypted is refused rather than sent in the
  clear.** On the desktop that means a Sonduit session against a target this
  process has not paired with -- an address typed by hand, or the multicast
  group -- does not start, and the button says why. There is no pairing this
  can strand: the paired-device list lives in the process and nothing persists
  it, so every session that has ever worked was paired inside the run that
  started it. Scream compatibility is the one exception, because that protocol
  has no version field and no key, and the panel says so for that session.
- **A user who wants an unencrypted sender has to say so.** On the phone,
  regenerating the pairing code discards the master secret with it, which is
  the one way back to a receiver that accepts version 1 or Scream. Stated in
  the UI rather than discovered.
- `docs/protocol.md` gains section 7.4.1 for the two handshake datagrams and
  section 8 for the sealed wire; the "version 2" and "Audio is still not
  encrypted" lines are gone. `docs/latency-budget.md` section 7 carries the
  measurement, outside the budget table.

## Alternatives rejected

- **A KDF over the six-digit code.** Rejected: 19.93 bits with an offline
  verifier already on the wire. It would look like encryption and would not be
  any. This is the alternative the whole ADR is written against.
- **A PAKE (SPAKE2, CPace).** The textbook answer, and it would remove the
  offline crack of the code entirely. Rejected for now because it buys almost
  nothing here: with an ephemeral Diffie-Hellman the cracked code already
  cannot decrypt anything, and the code is fresh per pairing, so what a PAKE
  protects is a value that is worthless by the time it is recovered. It costs a
  larger dependency and a protocol that is harder to get right. Revisit if
  codes ever become long-lived.
- **A 32-byte pre-shared key in the QR invite.** Genuinely strong -- the key
  would never touch the network at all -- and tempting, since the invite is
  delivered optically. Rejected because it only secures one of the two pairing
  paths: the typed six-digit path has no QR, and a design where one path is
  strong and the other looks identical but is not is worse than one honest
  design used everywhere.
- **XChaCha20-Poly1305 with a random 24-byte nonce.** Removes the counter
  question by making collisions merely improbable. Rejected: 12 more bytes on
  every packet to replace a guarantee with a probability, when the counter is
  already in the header and already unique.
- **AES-256-GCM.** Rejected on `armeabi-v7a`, which has no AES instructions.
- **A responder that answers each copy of an offer afresh.** Not so much
  rejected as found to be wrong: see section 1a. It is what shipped, and it
  agrees no key at all.
- **DTLS or WireGuard-style tunnelling.** Rejected: a handshake, a session
  state machine, a retransmission story and a large dependency, to protect four
  datagram kinds that already have a pairing exchange to key from.
- **Encrypt the header as well.** Rejected: the receiver must route, key and
  replay-check before it can authenticate, and the payload is a fixed size
  either way, so hiding the header would cost a second parse and buy nothing an
  observer could not infer from the traffic shape.
- **Leave the feedback datagram in the clear.** Rejected. It is four datagrams
  a second, it drives the numbers the user is shown and the sender's idea of
  whether the receiver is alive, and it would have been the only
  unauthenticated message left.

## What the tests cover, and what they did not

The defect reached hardware past a full test suite, and the reason is worth
recording because it is a pattern rather than an oversight:

- the transport's stand-in receiver answered every offer, as a real one does,
  but from a **constant seed**, so three copies were answered identically
  whether or not the responder remembered anything;
- the desktop's stand-in receiver **returned after the first offer**, so the
  repeats never met a responder at all.

Neither could distinguish a correct responder from one that minted a key pair
per datagram. Both now answer every copy from a **different** seed, and the
assertion is that audio sealed by one end opens at the other rather than that
two derived keys compare equal. Reverting the responder to its previous
behaviour turns six tests red across three crates.

## Revisit if

- A codec lands. Opus changes the packet size and the packet rate, and the
  per-packet cost above is measured against a 6 ms packet.
- Sonduit gains more than one concurrent receiver. One counter and one replay
  window per sender assumes one stream.
- Pairing codes ever become long-lived rather than per-session, at which point
  the offline crack starts to matter and a PAKE earns its cost.
