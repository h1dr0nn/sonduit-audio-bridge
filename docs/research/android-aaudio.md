# Android low-latency playback

Researched 2026-08-27.

**This document overturns one project assumption:** the plan named the `oboe`
Rust crate. It is stale, and `ndk::audio` is the maintained path.

---

## 1. Exclusive and low-latency mode are requests, not guarantees

`AAUDIO_SHARING_MODE_EXCLUSIVE` and `AAUDIO_PERFORMANCE_MODE_LOW_LATENCY` are
hints. **There is no error return when they are refused** — the open succeeds
with SHARED and/or `PERFORMANCE_MODE_NONE`. Everything must be read back:

```c
AAudioStream_getSharingMode(stream)
AAudioStream_getPerformanceMode(stream)
AAudioStream_isMMapUsed(stream)
AAudioStream_getFramesPerBurst(stream)
```

Exclusive streams are also more likely to be disconnected and should be closed
as soon as they are not needed.

## 2. Buffer sizing, and a trap

The burst size is fixed by hardware. Buffer size must be an integer multiple of
it; Oboe's default is two bursts.

**Do not enlarge the AAudio buffer to absorb network jitter.** Calling
`setBufferCapacityInFrames` with a large value inflates `framesPerBurst`
itself: a reported case saw a 2000 ms capacity produce a 48000-frame burst
(1000 ms). Google confirmed this is by design.

The correct architecture, and the one Sonduit already has, is a **second buffer
in front of AAudio** — our jitter buffer — with the AAudio buffer kept as small
as it will go.

## 3. Match the native sample rate, or pay 137 ms

Requesting a non-native rate routes audio through a slow resampling path.
Google's own measurements:

| Configuration | Round-trip |
| --- | --- |
| 44100 Hz requested from AAudio | **160 ms** |
| 44100 Hz with Oboe doing the conversion | 23 ms |
| Native rate, all recommendations followed | **20 ms** |

**Sonduit must send 48 kHz** and never ask AAudio for a rate the device does
not run natively.

## 4. Realistic latency

Every published Google figure is **round-trip**, not output-only.

| Configuration | Round-trip |
| --- | --- |
| LowLatency + Exclusive + native rate + callback | 20 ms |
| SHARED instead of EXCLUSIVE | 26 ms |
| Performance mode not LowLatency | **205 ms** |

Fleet-wide, Google measured a mean of 39 ms round-trip across popular phones in
January 2021, range 28-56 ms, down from 109 ms in 2017.

A Google engineer's informal estimate for the **output leg alone** is 15-40 ms
on a FAST track, and around 120 ms when off it.

**No output-only latency table exists for exclusive mode.** Sonduit's Android
output stage must be measured, not assumed.

## 5. The callback contract

From Oboe's `AudioStreamCallback.h`, the data callback must NOT: allocate, do
file or network I/O, use mutexes or other synchronisation primitives, sleep,
stop or close the stream, or read/write the stream.

**JNI is out.** The callback thread is created by the OS and is not attached to
the JVM. Attaching is possible but violates the realtime contract and the
attached thread lacks the app's ClassLoader. Google's guidance is to post an
event for another thread.

Data must cross from the network thread by a **non-blocking single-reader
single-writer FIFO**, which is AOSP's explicit recommendation for avoiding
priority inversion between the SCHED_FIFO callback and a normal-priority
producer. In Rust that means an audio-grade SPSC ring such as `rtrb`, never a
channel that can allocate or park.

Thread priority is not set directly: `PERFORMANCE_MODE_LOW_LATENCY` is what
gets the callback SCHED_FIFO. **ADPF** (`APerformanceHint_*`, API 33; Java API
31) lets the app declare its callback deadline and report actual duration; the
`ndk` crate gained bindings for it in March 2025.

## 6. OEM reality

Oboe's `QuirksManager.cpp` is the only code-level, Google-maintained list of
OEM audio defects:

| Vendor | Quirk |
| --- | --- |
| Samsung Exynos 9810, 8850 | Mono MMAP actually runs stereo |
| Samsung Exynos 990, 9810 | MMAP disabled for input on some builds |
| Samsung Exynos (all) | **Extra buffer margin**, so latency is structurally higher than on Qualcomm |
| Qualcomm SM8150 (Android P and below) | MMAP disabled entirely |

Reported device behaviour, from the Oboe issue tracker:

- **Huawei P20** refuses LowLatency+Exclusive; Google's Phil Burk: *"Huawei has
  disabled the FAST mixer path on many of their devices."*
- **MediaTek Helio A25/P35** budget SoCs: one developer's beta telemetry showed
  95% of affected devices reporting `framesPerBurst` of **960** (20 ms) with
  `PerfMode = NONE`. Note `PROPERTY_OUTPUT_FRAMES_PER_BUFFER` reported 256 on
  the same devices — **trust the opened stream, not the Java property.**
- **Galaxy S10/S10+ on Android 10:** audio crackling with MMAP output
  **whenever Wi-Fi is enabled**. Fixed in Android 11. Directly relevant, since
  Sonduit requires Wi-Fi.
- **Pixel 6a, Android 14:** audible glitch on every stream start in LowLatency
  mode; Google bug b/261782984, stale audio in the MMAP buffer. Mitigation:
  prime the ring with silence and ramp volume over the first few bursts.
- **Bluetooth never gets exclusive MMAP.** It is an ALSA-device feature.

No public per-device database of which devices grant exclusive mode exists.

## 7. Disconnection

`AAUDIO_ERROR_DISCONNECTED` means the stream **cannot be reused**; it must be
closed and a new one opened, and the new one may have a different rate and
burst size, so the ring buffer must be resized.

- An **error callback is mandatory**: with a data callback there is no return
  code to observe.
- **Never stop or close from the error callback thread** — deadlock. Oboe
  handles this by calling `onErrorAfterClose` on its own thread.
- On some Android 9 and early 10 builds, **disconnect messages never reach
  AAudio at all**. The documented `ACTION_HEADSET_PLUG` workaround is known to
  be incomplete for Bluetooth route changes (open since 2021). Add
  `registerAudioDeviceCallback` as a backstop, and a watchdog for the reported
  Huawei case where callbacks stop permanently while the stream claims Started.

## 8. Background execution

- **Android 14:** foreground services must declare a type; audio uses
  `mediaPlayback`.
- **Android 17 hardening applies to all apps regardless of target SDK.** Audio
  played without a visible activity or a qualifying foreground service is
  **silently suppressed** — `AAudioStream_write` plays silence, focus requests
  fail, volume APIs no-op, and no exception is thrown. A PC-to-phone bridge is
  exactly the profile this targets. The service must be started while the
  activity is visible.

## 9. The Rust binding: use `ndk::audio`, not `oboe`

| | `oboe` crate | `ndk` crate |
| --- | --- | --- |
| Latest release | 0.6.1, **2024-03-03** | actively released through 2026 |
| Bundled Oboe | 1.8.1 (upstream is 1.10.0) | n/a, binds AAudio directly |
| 2025-2026 activity | none merged | ongoing |
| `isMMapUsed` | **absent** | available |
| ADPF | **absent** | bindings added March 2025 |

`cpal` migrated from `oboe` to `ndk::audio` in 0.16.0 (June 2025), raising its
floor to API 26.

Choosing `ndk::audio` gives up Oboe's `QuirksManager` OEM workarounds and its
OpenSL ES fallback. The fallback is irrelevant at minSdk 27; the quirks would
have to be replicated only if a specific Exynos or Qualcomm bug is hit.

## 10. Minimum API level

**minSdk 27 (Android 8.1)**, which is where MMAP and exclusive mode became
real, covering about 94.8% of active devices as of April 2026. API 26 buys 1.3
points more on an AAudio implementation Google describes as having had critical
bugs.

Plan for graceful degradation regardless: a large minority of devices will
never grant LOW_LATENCY. Query `FEATURE_AUDIO_LOW_LATENCY` and
`FEATURE_AUDIO_PRO`, verify what was granted, and tell the user honestly.

## Sources

- https://developer.android.com/ndk/guides/audio/aaudio/aaudio
- https://developer.android.com/ndk/reference/group/audio
- https://developer.android.com/games/sdk/oboe/low-latency-audio
- https://developer.android.com/ndk/guides/audio/audio-latency
- https://developer.android.com/about/versions/17/changes/bg-audio
- https://developer.android.com/develop/background-work/services/fgs/service-types
- https://developer.android.com/ndk/reference/group/a-performance-hint
- https://source.android.com/docs/core/audio/aaudio
- https://source.android.com/docs/core/audio/avoiding_pi
- https://android-developers.googleblog.com/2021/03/an-update-on-androids-audio-latency.html
- https://github.com/google/oboe/blob/main/include/oboe/AudioStreamCallback.h
- https://github.com/google/oboe/blob/main/src/common/QuirksManager.cpp
- https://github.com/google/oboe/wiki/TechNote_BufferTerminology
- https://github.com/google/oboe/wiki/TechNote_Disconnect
- https://github.com/google/oboe/issues/996 (Huawei P20)
- https://github.com/google/oboe/issues/1178 (Galaxy S10 Wi-Fi crackling)
- https://github.com/google/oboe/issues/1254 (MediaTek burst sizes)
- https://github.com/google/oboe/issues/1411 (buffer capacity inflates burst)
- https://github.com/google/oboe/issues/1468 (incomplete disconnect detection)
- https://github.com/google/oboe/issues/1842 (Pixel 6a start glitch)
- https://github.com/google/oboe/issues/2381 (Huawei callbacks stop)
- https://crates.io/crates/oboe
- https://github.com/katyo/oboe-rs
- https://docs.rs/ndk/latest/ndk/audio/index.html
- https://github.com/RustAudio/cpal/blob/master/CHANGELOG.md
- https://apilevels.com/

## Not verified

1. **Output-only latency for exclusive MMAP.** Every published figure is
   round-trip. Sonduit's Android output stage must be measured.
2. No public database of which devices grant exclusive mode.
3. Xiaomi/MIUI/HyperOS as an OS-level policy: **no evidence found.** The Xiaomi
   devices in the burst-size table appear there because of their SoC, and that
   table is one developer's telemetry in a GitHub issue.
4. CPU affinity for audio threads: no AAudio or Oboe API found, and no official
   guidance recommending it.
5. Whether the `oboe` Rust crate still builds against current NDK and Rust. Its
   CI has not run since early 2024. Not attempted.
6. ADPF availability across OEMs.
7. Whether exclusive MMAP is ever available over Bluetooth. Stated as a strong
   inference from the ALSA/dmabuf mechanism, not a documented fact.
8. Android 17 hardening details come from a preview page and may shift.
