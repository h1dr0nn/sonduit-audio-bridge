# Roadmap

Ordered by risk, not by convenience. The things that could invalidate the plan
come first, because finding out late is what makes them expensive.

Nothing has shipped. The tree is at 1.1.0, no `release-v*` tag exists, and the
only release this repository has ever published is Harmonix's `v1.0.4` -- a
different product, kept as history. The list below is what stands between here
and a first Sonduit release.

---

## 0. Blocked on the maintainer

These cannot be done from a development machine and are not code problems.

| Item | Why it is blocked |
| --- | --- |
| **Measure latency on the phone** | Audio has been heard on a real device over USB tethering, so the largest unknown is gone. Nothing has been *measured*: no loopback-cable timing, no round-trip figure, and no record of whether AAudio granted exclusive low-latency mode. Every figure in `latency-budget.md` is still arithmetic. The app already reports the granted sharing mode and the burst size, so one session on hardware answers most of it. |
| **Re-test after the swing and crackle fixes** | Three fixes landed after the session that found them -- the wired-link flag, tethered-adapter detection, and pacing the jitter-buffer drain -- and none has been run on the phone. See the README status section. |
| **Hear it over Wi-Fi on a second access point** | Cleared once, on 2026-09-04: eight runs on the reporter's AP, audio on Wi-Fi with the cable carrying adb only. One radio is one data point, and it is the radio the code was tuned against. |
| **Test tethering on real OEM builds** | Carrier entitlement can veto USB tethering, and one phone is one data point. Samsung, Xiaomi and OPPO, with and without a SIM. The biggest remaining risk to ADR-004. |
| **Android release keystore** | Release APK signing. A credential this repository must never hold; until it exists `release.yml` publishes an unsigned APK on purpose, and says so in the release notes. |
| **Resolve the driver signing question** | Whether an attestation-signed kernel driver still loads under the April 2026 policy decides whether Tier 2 of ADR-002 is weeks or months. Needs Microsoft Partner Center support. |
| **Obtain an EV code-signing certificate** | Mandatory even to register for the Hardware Developer Program, and the prerequisite for the item above. A paid credential. |
| **Tauri updater keypair** | The updater was removed from `tauri.conf.json` rather than left pointing at Harmonix's public key, which would have accepted updates signed by the old key. Verified: `tauri.conf.json` has no `updater` block and no `pubkey`. Re-enable once a Sonduit minisign keypair exists. |

---

## 1. Open work that is not blocked

### 1.1 Process loopback is not implemented

`sonduit-capture-win` returns `CaptureError::ModeUnavailable` for
`CaptureMode::ProcessLoopback`. Endpoint loopback works and is what the product
uses today. This is a declared error, not a silent no-op, so it fails loudly
if anything selects it.

### 1.2 A pairing does not survive the desktop being closed

The list of paired devices lives in the desktop process and nothing writes it
down, so quitting the application discards every key with it and the user pairs
again next time. That was free before ADR-009 and is not free now: a pairing is
what a session is keyed from.

Persisting it is not simply a file. A master secret on disk needs somewhere to
live that is not a plain text file in the user's profile, which on Windows
means DPAPI, and it needs a way for the user to see and revoke what is stored.
Neither is written.

### 1.3 A restarted sender can leave the receiver deaf for a minute

Observed once, on 2026-09-04, and not reproduced since. A sender was restarted
against a receiver that had never stopped, so its sequence numbers and its
timestamp base began again while the receiver's state carried on. The phone then
sat at **`depth 0 + queued 0` while accepting every packet at full rate**, with
the drift estimate reading **up to 5.2 billion ppm**, for about **60 seconds**,
after which it healed itself with no intervention.

What healed it is not known, and nothing is written that was meant to. The
buffer has a `reset` for a new sender, `resync_if_hopeless` for a runaway queue,
and a two-second gap rule that discards drift history; which of them fired, if
any, was not captured. A minute of silence that the receiver reports as a
perfectly healthy session is the worst failure shape this project has, because
nothing in the telemetry says anything is wrong.

Wanted, in order: the log from a second occurrence, a synthetic test in
`sonduit-core` that restarts the sequence and timestamp base under a running
buffer, and only then a fix. It is written down here because one observation is
not enough to fix from and is easily lost.

### 1.4 Underrun cannot be measured on hardware

`PlaybackCounters::frames_underrun` is incremented in the AAudio callback
(`crates/sonduit-playback-android/src/aaudio.rs`) and **read by nothing**. Not
the FFI telemetry, which reads `frames_played` out of the same struct and stops
there; not the receiver's log line; not the phone's UI; not the feedback report
the sender's panel renders.

So **every underrun claim this project has made is either a harness figure or a
proxy**. The proxy is `depth 0 + queued 0` in a log line printed once every 40
packets, which is 240 ms: an empty-queue indication sampled four times a second,
which cannot tell a callback that wrote one buffer of silence from one that
wrote forty. "Underrun is unchanged at ceilings of 200, 120, 80 and 60 ms" is a
statement about `examples/jitter_timeline.rs` and has never been checked on a
device.

The counter, the percentage helper and the atomics are already written. What is
missing is a path from `PlaybackCounters` into the feedback report and the log
line, beside `frames_played`. Until that exists, the quality metric this product
is judged on is the one metric it does not report.

---

## 1a. Already cleared

Kept here rather than deleted, so the lists above are read as what remains and
not as everything there ever was.

| Was | Now |
| --- | --- |
| Nothing had ever been heard | Audio played on a real phone over USB tethering |
| A dead capture device ended the session | The client is replaced in place; verified against a live endpoint |
| Discovery had no authentication | Pairing code, HMAC over a per-probe nonce, verified over real sockets |
| The audio went out in the clear | ChaCha20-Poly1305 per packet, keyed by an X25519 exchange the pairing code authenticates. A keyed receiver refuses cleartext and an unkeyed one refuses sealed; see ADR-009 |
| One buffer depth for every link | `JitterConfig::for_transport`, and the sender now declares the link in the packet header rather than leaving the receiver to guess from the address |
| The tethered phone had to be typed in | Adapter enumeration reads the gateway from the routing table; probes go to it directly |
| Tether detection matched no real device | "Remote NDIS" is two words and `UsbNcm` is one; the tokens now match both |
| The buffer target chased the jitter estimate | Asymmetric retargeting with a 1.7x shrink threshold and cooldowns |
| The target shrank rarely and violently | The 1.7 was compared against a suggestion the configuration had already floored, so on Wi-Fi only a target above 51 ms could satisfy it. Measured on a real access point, the old build managed three shrinks in fifteen minutes, each a single 36 ms jump; the corrected code, with roc's floor on the target and roc's proportional step, makes 48 downward moves against 44 upward and walks (`80 -> 64 -> 50 -> 40 -> 32 -> 30`). "Structurally zero" was true of the harness, not of the link |
| A Wi-Fi receiver could hold 200 ms | 80 ms, the top of the band `latency-budget.md` targets. Measured on the access point that prompted it, sender end-to-end p95 fell from 100.6 ms to 79.2 ms. The reason first recorded -- that 200 ms was where the depth settled -- is withdrawn: on that radio the depth held p50 30 / p95 60 ms with the old ceiling in place. Whether underrun changed is unknown, see section 1.4; see ADR-004 |
| Nothing had been heard, let alone measured, over Wi-Fi | Eight 300 s runs on the reporter's own access point, cable in for adb only with tethering off. `latency-budget.md` section 6 and `research/jitter-and-drift.md`. One AP, one phone, unpaired before-and-after, and still nothing timed against a loopback cable |
| Drift history survived a sleep or a route change | A gap of two seconds discards it, and the correction with it |
| The installer left the firewall shut | Withdrawn rather than cleared. The NSIS hook is deleted: a per-user install has no rights for `netsh`, and a port-scoped rule cannot suppress a prompt Windows raises per program. Windows asks once on first run; see ADR-004 |
| The audio callback took a mutex | The handoff is lock-free; the callback holds one half and takes no lock at all |
| One packet in drained up to three out | One out per one in, plus a little to make up a short queue |
| A release needed a branch and a pull request | A tag, and nothing else |

---

## 2. Known gaps in what is already written

Honest list of things that exist but are not finished.

| Gap | Where |
| --- | --- |
| Process loopback returns `ModeUnavailable`; only endpoint loopback works | `sonduit-capture-win`, see section 1.1 |
| A pairing lasts only as long as the desktop process, so every run pairs again | see section 1.2 |
| The bundled FFmpeg is 110 MB installed, about 35 MB inside the LZMA installer; no smaller LGPL build is published | `tools/fetch-ffmpeg.mjs` |
| `driver/` does not exist in the tree at all | ADR-002 |
| The About screen names FFmpeg and points at `FFMPEG-LICENSE.txt` only; `THIRD-PARTY-LICENSES.txt` is installed but not signposted | `docs/licensing.md` section 5.1 |
| **Buffer depth past the hand-off ring protects nothing.** The ring is refilled on arrival, so during a stall the callback drains its five packets and then plays silence however deep the jitter buffer is. The figure behind it -- underrun identical at ceilings of 200, 120, 80 and 60 ms -- is a harness result and cannot currently be checked on a device; see section 1.4 | `sonduit-core::pacing`, `research/jitter-and-drift.md` |
| **The target is close to blind to the failure Wi-Fi has.** RFC 3550's estimator is a mean absolute first difference, and a stall is one large difference followed by a run of small ones: simulated 120 ms stalls every 12 s moved the estimate to 9.3 ms and the target to 34. roc's `MAX(peak * 1.2, mean * 3.0)` has a peak term for this and Sonduit has only the mean. Unconfirmed on hardware: the measured access point ran an estimate of 3.2-6.4 ms and was never seen to stall that way | `sonduit-core::jitter`, `research/jitter-and-drift.md` |
| **Underrun is incremented and read by nothing**, so the one quality figure this product is judged on is the one it does not report | `sonduit-playback-android`, section 1.4 |
| **A sender restart can strand the receiver at `depth 0 + queued 0` for a minute** while it accepts every packet, with drift reading billions of ppm. Seen once, not reproduced, no test | `sonduit-core::jitter`, `sonduit-ffi`, section 1.3 |

---

## 3. Deferred by design

### `AudioProcessor`

`sonduit-core::processor::AudioProcessor` is a trait with **no implementation
anywhere in the tree**. It is the seat between capture and transport for
realtime DSP: EQ, gain, compression. `RingBuffer::with_contiguous_mut` exists
so that work can happen in place rather than through a scratch buffer.

Defining the seat now is cheap. Retrofitting one through a shipped audio path
is not.

### Opus

The wire format sends raw PCM. A codec belongs behind a trait, and that trait
does not exist yet either. 1.536 Mbit/s is fine on a LAN and on USB; it stops
being fine on a congested access point.

### Windows as an output endpoint

ADR-002 Tier 2. Until a signed driver exists, Sonduit is a loopback bridge and
the README says so.

---

## 4. Placeholders

CONTRIBUTING requires every `todo!()` in the Rust tree to be listed here.

**There are none.** Checked across `crates/` and `desktop/src-tauri/src/`:
`grep -rn "todo!" --include="*.rs"` returns nothing, and neither does the same
search for `unimplemented!`.

The two things that are deliberately not implemented are not silent and are not
placeholders either. `CaptureMode::ProcessLoopback` returns a declared
`ModeUnavailable` error, and `AudioProcessor` is a trait with no implementor,
which is a compile-time absence rather than a runtime one. Both are recorded in
sections 1.2 and 3 above.

If a `todo!()` is ever added, it belongs in this section in the same commit.

---

## 5. Repository hygiene

- Set `main` protection. There is one branch now and nothing is built by
  pushing to it, but `main` is still writable directly, and every change is
  meant to arrive through a short-lived `feat/*`, `fix/*` or `chore/*` branch
  and a pull request.
- Decide whether to delete the Harmonix `v1.0.4` GitHub Release. The four
  releases up to `v1.0.3` were removed as instructed; `v1.0.4` was not named
  and its assets are not recoverable, so it was left alone. The Harmonix git
  tags themselves stay: ADR-008 makes `harmonix-final` load-bearing for the
  changelog range and for the last-release lookup.
- `tools/cliff.toml` still carries the old release tag format.
  `tag_pattern = "^v[0-9]+\.[0-9]+\.[0-9]+$"` matches the Harmonix tags
  `v1.0.0` to `v1.0.4` and matches no Sonduit release tag, which is the
  opposite of what the comment above it claims; the comment also calls the
  rolling prerelease `develop` rather than `develop-build`. Nothing is broken
  today, because `release.yml` always passes an explicit
  `harmonix-final..<tag>` range, but the changelog heading template is
  `{{ version | trim_start_matches(pat="v") }}`, which would render a
  `release-v2.1.0` tag as `release-v2.1.0` instead of `2.1.0`. Untested here
  because it only runs inside `release.yml`, so it is left for the maintainer
  to change deliberately.
