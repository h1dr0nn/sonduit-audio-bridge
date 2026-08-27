# Roadmap

Ordered by risk, not by convenience. The things that could invalidate the plan
come first, because finding out late is what makes them expensive.

---

## 0. Blocked on the maintainer

These cannot be done from a development machine and are not code problems.

| Item | Why it is blocked |
| --- | --- |
| **Resolve the driver signing question** | Whether an attestation-signed kernel driver still loads under the April 2026 policy decides whether Tier 2 of ADR-002 is weeks or months. Needs Microsoft Partner Center support. |
| **Obtain an EV code-signing certificate** | Mandatory even to register for the Hardware Developer Program. A paid credential. |
| **Android release keystore** | Release APK signing. A credential this repository must never hold. |
| **Tauri updater keypair** | The updater was removed from `tauri.conf.json` rather than left pointing at Harmonix's public key, which would have accepted updates signed by the old key. Re-enable once a Sonduit minisign keypair exists. |
| **Test tethering on real OEM builds** | Carrier entitlement can veto USB tethering. Samsung, Xiaomi and OPPO, with and without a SIM. The biggest risk to ADR-004. |
| **Measure anything** | No Android device exists here. Every figure in `latency-budget.md` is a budget. |

---

## 1. Highest-risk unknowns, resolve first

1. **Does the Android output stage fit the budget?**
   `latency-budget.md` allots 16 ms to stages 8 and 9 combined. Published
   evidence puts the output leg anywhere from 15 to 120 ms. If it lands high,
   the 40-80 ms Wi-Fi target is unreachable and the product promise changes.
   *Resolution: an APK that opens a stream, reports the granted mode and burst
   size, and measures round-trip with a loopback cable.*

2. **Does USB tethering work reliably on consumer phones?**
   See above. If entitlement blocks it widely, the USB path needs AOA, which
   has an unsolved Windows driver problem.

3. **Can we ever show as a Windows output endpoint?**
   ADR-002 Tier 2. Until then Sonduit is a loopback bridge, and the README says
   so.

---

## 2. Next implementation milestones

### 2.1 Wire the core into the desktop shell

`useBridge` currently reports `available: false` and every panel renders its
empty state. This is the first thing a user would notice.

- Tauri commands to start and stop a session.
- A telemetry event stream from Rust into the webview, feeding `Telemetry`.
- Replace `useBridge`'s body with a subscription. The shape it returns is
  already the shape the event will carry.

### 2.2 WASAPI capture (ADR-002 Tier 1)

- `enumerate_endpoints` and `open` are `todo!()`.
- Process loopback on Windows 11; endpoint loopback plus a silent render
  keepalive on Windows 10.
- Verified only on GitHub Actions `windows-latest` and by the maintainer.

### 2.3 The Android app

- Gradle project, Compose UI, `mediaPlayback` foreground service started while
  the activity is visible (Android 17 hardening).
- `sonduit-playback-android` on `ndk::audio`.
- UniFFI interface definition and binding generation in `sonduit-ffi`.
- CI already cross-compiles the Rust; the APK step is gated on
  `android/settings.gradle` existing and will light up on its own.

### 2.4 Transport hardening

- **Transport-aware buffer depth.** ADR-004: one constant either wastes 18 ms
  on USB or underruns on Wi-Fi.
- Windows adapter enumeration to find the RNDIS or NCM interface and read its
  DHCP gateway. Never hardcode an address.
- Installer adds an inbound firewall rule; the tether adapter lands on the
  Public profile which blocks all inbound.
- Manual address entry, for networks that block broadcast.

### 2.5 Jitter buffer, second pass

The current buffer implements RFC 3550 and adapts, but is missing what the
research says matters most:

- **Asymmetric retargeting: grow fast, shrink slowly and rarely.** roc uses a
  1.7x threshold, a 5 s cooldown after a decrease and 15 s after an increase.
  Symmetric adjustment oscillates.
- A **percentile** statistic alongside the mean. NetEq reads a 0.95 quantile
  from a forgetting histogram; the RFC's own text says its estimator "is not
  intended to be taken quantitatively".
- Better concealment than silence. Currently a lost packet writes zeroes.

### 2.6 Drift correction

`DriftEstimator` measures drift but nothing acts on it. At 50 ppm with 30 ms of
headroom the buffer runs dry in **10 minutes**, so this is not optional.

- ASRC through `rubato`'s `Async` resampler, ratio nudged by a PI controller,
  ramped rather than stepped.
- Reset the estimator on route change, suspend, or a large time gap.
- Sample drop and insert as the emergency path only.

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

---

## 4. Known gaps in what is already written

Honest list of things that exist but are not finished:

| Gap | Where |
| --- | --- |
| `enumerate_endpoints`, `open` are `todo!()` | `sonduit-capture-win` |
| `Bridge::start`, `Bridge::stop` are `todo!()` | `sonduit-ffi` |
| Playback `open` is `todo!()` | `sonduit-playback-android` |
| No aggregated third-party licence file is shipped | `licensing.md` section 5 |
| FFmpeg LGPL notice is not shown in About, and no LGPL text ships in the bundle | `licensing.md` section 2.2 |
| Mastering uses a single loudnorm pass; two-pass would be more accurate | `convert/args.rs` |
| The bundled FFmpeg is 110 MB, which dominates the installer size | `tools/fetch-ffmpeg.mjs` |
| Discovery has **no authentication or pairing** | ADR-006. Not acceptable for a shipped product |
| The Scream driver commit is not pinned | `licensing.md`; a floating reference is not good enough |
| `driver/` is empty | ADR-002 |
| Locale strings cover the current shell only | New screens need new keys |

---

## 5. Repository hygiene

- Create the `develop` branch and set `main` protection. The branch model in
  ADR-008 is documented but not yet configured on the remote.
- Decide whether to delete the Harmonix `v1.0.4` GitHub Release. The four
  releases up to `v1.0.3` were removed as instructed; `v1.0.4` was not named
  and its assets are not recoverable, so it was left alone.
