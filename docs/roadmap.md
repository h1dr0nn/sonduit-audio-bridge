# Roadmap

Ordered by risk, not by convenience. The things that could invalidate the plan
come first, because finding out late is what makes them expensive.

---

## 0. Blocked on the maintainer

These cannot be done from a development machine and are not code problems.

| Item | Why it is blocked |
| --- | --- |
| **Test on a real phone** | Nothing here has ever been heard. Whether AAudio grants exclusive low-latency mode, what burst size a real device reports, and every figure in `latency-budget.md` are all unknown until an APK runs on hardware. This is the single largest gap. |
| **Test tethering on real OEM builds** | Carrier entitlement can veto USB tethering. Samsung, Xiaomi and OPPO, with and without a SIM. The biggest risk to ADR-004. |
| **Android release keystore** | Release APK signing. A credential this repository must never hold; until it exists the release workflow produces an unsigned artifact on purpose. |
| **Resolve the driver signing question** | Whether an attestation-signed kernel driver still loads under the April 2026 policy decides whether Tier 2 of ADR-002 is weeks or months. Needs Microsoft Partner Center support. |
| **Obtain an EV code-signing certificate** | Mandatory even to register for the Hardware Developer Program. A paid credential. |
| **Tauri updater keypair** | The updater was removed from `tauri.conf.json` rather than left pointing at Harmonix's public key, which would have accepted updates signed by the old key. Re-enable once a Sonduit minisign keypair exists. |

---

## 1. Before this can be called a release

Each one is a reason a 2.0.0 tag would be dishonest today.

### 1.1 Nothing has been measured

`latency-budget.md` is arithmetic. Until an APK runs on a phone with a loopback
cable, the 40-80 ms target is a hypothesis and not a claim. The app already
reports the granted sharing mode and the burst size, so the first run on real
hardware answers most of it.

This is the only item on this list that cannot be worked on from here.

### 1.2 The audio itself is not encrypted

Pairing stops an unpaired device being *selected*, which was the eavesdropping
bug. It does nothing about anyone who can already see the traffic: the PCM is
in the clear and reconstructing it is trivial.

The README now states this plainly, which is the minimum. A cipher keyed from
the pairing exchange is the real answer and is not written.

---

## 1a. Release blockers already cleared

Kept here rather than deleted, so the list above is read as what remains and
not as everything there ever was.

| Was | Now |
| --- | --- |
| A dead capture device ended the session | The client is replaced in place; verified against a live endpoint |
| Discovery had no authentication | Pairing code, HMAC over a per-probe nonce, verified over real sockets |
| One buffer depth for every link | `JitterConfig::for_transport`, chosen from the sender's address |
| The tethered phone had to be typed in | Adapter enumeration reads the gateway; probes go to it directly |
| The installer left the firewall shut | NSIS hook adds an inbound rule for the discovery port on all three profiles |

## 2. Known gaps in what is already written

Honest list of things that exist but are not finished.

| Gap | Where |
| --- | --- |
| Process loopback returns `ModeUnavailable`; only endpoint loopback works | `sonduit-capture-win` |
| The jitter buffer adapts symmetrically; roc uses a 1.7x threshold with asymmetric cooldowns | `sonduit-core::jitter` |
| A lost packet is concealed with silence, not with anything better | `sonduit-core::jitter` |
| The drift estimator is not reset on route change or suspend, only on format change | `sonduit-ffi` |
| Discovery is authenticated but the audio stream is not encrypted | ADR-006, see section 1.2 |
| No aggregated third-party licence file is shipped | `licensing.md` section 5 |
| Mastering uses a single loudnorm pass; two-pass would be more accurate | `convert/args.rs` |
| The bundled FFmpeg is 110 MB, which dominates the installer size | `tools/fetch-ffmpeg.mjs` |
| The Scream driver commit is not pinned | `licensing.md`; a floating reference is not good enough |
| `driver/` is empty | ADR-002 |

---

## 3. Deferred by design

### `AudioProcessor`

`sonduit-core::processor::AudioProcessor` is defined and **not implemented**.
It is the seat between capture and transport for realtime DSP: EQ, gain,
compression. `RingBuffer::with_contiguous_mut` exists so that work can happen
in place rather than through a scratch buffer.

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

## 4. Repository hygiene

- Set `main` protection. The `develop` branch exists and publishes rolling
  builds, but `main` is still writable directly, which the branch model in
  ADR-008 says it should not be.
- Decide whether to delete the Harmonix `v1.0.4` GitHub Release. The four
  releases up to `v1.0.3` were removed as instructed; `v1.0.4` was not named
  and its assets are not recoverable, so it was left alone.
