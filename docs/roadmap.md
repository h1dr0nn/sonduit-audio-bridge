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

Ordered. Each one is a reason a 2.0.0 tag would be dishonest today.

### 1.1 Discovery has no authentication

ADR-006. Any device on the network can answer a probe, and the desktop will
start sending system audio to whatever answered first. On a shared network that
is an eavesdropping bug, not a missing feature.

Minimum: a pairing code shown on the phone and typed on the desktop, mixed into
a key that authenticates the announce reply. Encrypting the audio itself is a
separate and larger decision.

### 1.2 Nothing has been measured

`latency-budget.md` is arithmetic. Until an APK runs on a phone with a loopback
cable, the 40-80 ms target is a hypothesis. The app already reports the granted
sharing mode and burst size, so the first run answers most of it.

### 1.3 Transport-aware buffer depth

ADR-004: one constant either wastes 18 ms on USB or underruns on Wi-Fi. The
sender already labels the link, so the receiver can be told which it is.

### 1.4 The tethered interface is not found automatically

Windows adapter enumeration to locate the RNDIS or NCM interface and read its
DHCP gateway. Today the user types an address. Never hardcode 192.168.42.129:
the range is conventional, not guaranteed.

### 1.5 The installer does not open the firewall

The tether adapter lands on the Public profile, which blocks all inbound. A
first run over USB will look like a total failure with no diagnostic.

---

## 2. Known gaps in what is already written

Honest list of things that exist but are not finished.

| Gap | Where |
| --- | --- |
| Process loopback returns `ModeUnavailable`; only endpoint loopback works | `sonduit-capture-win` |
| A capture device that disappears is reported but not recovered from | `desktop/src-tauri/src/bridge` |
| The jitter buffer adapts symmetrically; roc uses a 1.7x threshold with asymmetric cooldowns | `sonduit-core::jitter` |
| A lost packet is concealed with silence, not with anything better | `sonduit-core::jitter` |
| The drift estimator is not reset on route change or suspend, only on format change | `sonduit-ffi` |
| No aggregated third-party licence file is shipped | `licensing.md` section 5 |
| FFmpeg LGPL notice is not shown in About, and no LGPL text ships in the bundle | `licensing.md` section 2.2 |
| Mastering uses a single loudnorm pass; two-pass would be more accurate | `convert/args.rs` |
| The bundled FFmpeg is 110 MB, which dominates the installer size | `tools/fetch-ffmpeg.mjs` |
| The Scream driver commit is not pinned | `licensing.md`; a floating reference is not good enough |
| `driver/` is empty | ADR-002 |
| Android strings are English only, while the desktop has eleven languages | `android/app/src/main/res/values` |

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

- Create the `develop` branch and set `main` protection. The branch model in
  ADR-008 is documented but not yet configured on the remote.
- Decide whether to delete the Harmonix `v1.0.4` GitHub Release. The four
  releases up to `v1.0.3` were removed as instructed; `v1.0.4` was not named
  and its assets are not recoverable, so it was left alone.
