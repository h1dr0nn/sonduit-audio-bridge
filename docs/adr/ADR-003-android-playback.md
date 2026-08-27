# ADR-003: Android audio output engine

- Status: accepted, with **one correction to the original plan**
- Date: 2026-08-27

## Context

The Android receiver must play a network-fed PCM stream at the lowest latency
the device will allow. The plan named AAudio via the `oboe` Rust crate, with
Kotlin and Compose calling into Rust through UniFFI, and the audio callback
staying entirely in native code.

Research confirms the architecture and **rejects the crate**. See
`research/android-aaudio.md`.

## Decision

### AAudio, requesting exclusive low-latency mode, verifying what was granted

`AAUDIO_SHARING_MODE_EXCLUSIVE` and `AAUDIO_PERFORMANCE_MODE_LOW_LATENCY` are
requests. **AAudio does not fail an open when it cannot honour them** - it
succeeds with something worse. `GrantedStream` in
`sonduit-playback-android` therefore carries what was actually granted, read
back from the opened stream, and the UI reports it honestly rather than
claiming a low-latency path the device never provided.

### `ndk::audio`, not the `oboe` crate

| | `oboe` crate | `ndk` crate |
| --- | --- | --- |
| Latest release | 0.6.1, **March 2024** | actively released through 2026 |
| Bundled Oboe | 1.8.1 (upstream 1.10.0) | binds AAudio directly |
| `isMMapUsed` | **absent** | available |
| ADPF bindings | **absent** | added March 2025 |

`cpal` made the same migration in 0.16.0. Adopting a two-and-a-half-year-old
unmaintained dependency with no visibility into whether the MMAP path was
actually granted is not defensible for a latency-critical product.

The cost is giving up Oboe's `QuirksManager` OEM workarounds and its OpenSL ES
fallback. The fallback is irrelevant at minSdk 27. The quirks would have to be
replicated only if a specific device bug is hit, and the list is short enough
to port if that happens.

### minSdk 27

Android 8.1 is where MMAP and exclusive mode became real, and it covers about
94.8% of active devices. API 26 buys 1.3 points more on an AAudio
implementation Google itself describes as having had critical bugs.

### The callback owns nothing but a ring buffer

The data callback must not allocate, lock, sleep, do I/O, or touch the stream.
**It must never call JNI**: the callback thread is created by the OS, is not
attached to the JVM, and attaching both violates the realtime contract and
loses the app's ClassLoader.

The network thread writes into a lock-free SPSC ring; the callback drains it
and emits silence on underrun. This is AOSP's explicit recommendation for
avoiding priority inversion between the SCHED_FIFO callback and a
normal-priority producer.

`sonduit-core::ring::RingBuffer` is that buffer, and `read_or_silence` is that
operation.

### Buffer sizing: small in AAudio, deep in our own buffer

**Do not enlarge the AAudio buffer to absorb network jitter.** A large
`setBufferCapacityInFrames` inflates `framesPerBurst` itself; one report saw a
2000 ms capacity produce a 1000 ms burst, and Google confirmed that is by
design.

AAudio stays at two bursts, three if xruns appear. **All jitter absorption
happens in our jitter buffer**, which is where it can be measured and tuned.

### Send 48 kHz and never ask for a rate the device does not run

Requesting a non-native rate costs, by Google's own measurement, **160 ms
round-trip instead of 20 ms**. This is the largest single configuration
mistake available and it is avoided by construction.

## Consequences

- The UI must be able to say "high latency mode" honestly. `GrantedStream`
  exists for that.
- Disconnection handling is not optional. `AAUDIO_ERROR_DISCONNECTED` means the
  stream cannot be reused; a new one may have a different rate and burst size,
  so the ring buffer must be re-sized on reconnect. An error callback is
  mandatory, and stopping the stream from inside it deadlocks.
- Known hazards to design around: the Pixel 6a start glitch (prime with silence
  and ramp), Samsung Exynos carrying extra buffer margin, MediaTek Helio
  devices reporting 960-frame bursts and never granting low latency, and the
  Galaxy S10 crackling that appears **only when Wi-Fi is on** - which Sonduit
  always requires.
- **Android 17 background audio hardening applies to all apps regardless of
  target SDK**, and violations are silently suppressed rather than throwing. A
  `mediaPlayback` foreground service must be started while the activity is
  visible.

## Not verified

No output-only latency figure exists for exclusive MMAP; every published Google
number is round-trip. Sonduit's Android output stage must be measured on real
hardware before any figure in `latency-budget.md` is quoted as a result.
