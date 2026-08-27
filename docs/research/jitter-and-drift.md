# Jitter buffering and clock drift

Researched 2026-08-27. This one **confirms** the project's assumptions and
supplies the constants.

---

## A. RFC 3550 inter-arrival jitter

### The estimator, verbatim

From RFC 3550 section 6.4.1:

> If Si is the RTP timestamp from packet i, and Ri is the time of arrival in
> RTP timestamp units for packet i, then for two packets i and j, D may be
> expressed as
> ```
> D(i,j) = (Rj - Ri) - (Sj - Si) = (Rj - Sj) - (Ri - Si)
> ```
> The interarrival jitter SHOULD be calculated continuously as each data packet
> i is received [...] according to the formula
> ```
> J(i) = J(i-1) + (|D(i-1,i)| - J(i-1))/16
> ```

Appendix A.8 gives the reference implementation:

```c
int transit = arrival - r->ts;
int d = transit - s->transit;
s->transit = transit;
if (d < 0) d = -d;
s->jitter += (1./16.) * ((double)d - s->jitter);
```

`transit` is the relative transit time: true network delay plus an unknown
constant clock offset. `D` is its first difference, which cancels that offset.
**This is exactly what `crates/sonduit-core/src/jitter.rs` implements.**

### What the 1/16 gain does

An EWMA with `a = 1/16`:

- **Time constant:** `1 / -ln(0.9375) = 15.5 packets`. At Sonduit's 6 ms
  packets that is 93 ms.
- **Noise reduction:** output variance is `a/(2-a) = 0.0323` times input, i.e.
  a 5.6x reduction in standard deviation.

RFC 3550 section 6.4.4 warns the value *"is not intended to be taken
quantitatively"* — it is for comparison over time, not an absolute delay bound.
That is why serious implementations also track a peak or percentile.

### Turning it into a playout delay

The classic rule is Ramjee et al., INFOCOM 1994: `p = t + d + 4*v`, i.e.
**k = 4** on a mean absolute deviation, with a very slow filter
(alpha = 0.998002, about 500 packets).

**roc-toolkit**, the most directly comparable project, uses:

```
estimation = MAX(peak_jitter * 1.2, mean_jitter * 3.0)
```

with `latency_decrease_relative_threshold = 1.7`, a 5 s startup timeout, a 5 s
cooldown after a decrease and **15 s after an increase**.

> **Grow fast, shrink slowly and rarely.** This asymmetry is the single most
> important design rule in the area, and Sonduit's buffer does not yet
> implement it. Tracked in the roadmap.

roc also switches profile at 30 ms: below that it uses a "responsive" profile
because *"gradual profile could cause oscillations comparable with the latency
and break playback"* — directly relevant to Sonduit's 40-80 ms target.

### WebRTC NetEq

NetEq does **not** use mean+k*variance. It builds a **forgetting histogram** of
relative arrival delays in 20 ms buckets and reads the **0.95 quantile**:

```
quantile = 0.95, forget_factor = 0.983, resample_interval_ms = 500
kStartDelayMs = 80, ms_per_loss_percent = 20
```

The signal path is: relative delay -> max over each 500 ms window -> 20 ms
bucket -> forgetting histogram -> 95th percentile -> `target = (bucket+1)*20ms`.

Its architectural trick is worth noting: **arrival time is measured in units of
its own 10 ms output ticks**, so sender/receiver drift folds into the delay
statistic automatically instead of needing a separate loop.

Buffer adjustment is pitch-synchronous overlap-add: it finds the strongest
autocorrelation peak in a 2.5-15 ms lag range and inserts or removes exactly
one pitch period with a cross-fade, **refusing to do so unless correlation
exceeds a threshold or the frame is inactive speech.** That refusal gate is
what prevents artefacts.

### Expected jitter on real links

| Link | Realistic one-way jitter |
| --- | --- |
| Wired / USB tether | < 1 ms p99 |
| 5 GHz Wi-Fi, uncongested | ~1 ms median, few ms p95 |
| 5 GHz Wi-Fi, loaded AP | 10-20 ms p95 |
| **2.4 GHz Wi-Fi, typical apartment** | ~6 ms median, ~35 ms p95, **250 ms p99** |

Sources: a MobiSys 2016 campus WLAN study measured 3 / 20 / 250 ms at the 50th,
90th and 99th percentiles. Grigorik measured 6.22 ms median and 34.87 ms p95 on
2.4 GHz versus 0.90 ms and 1.58 ms on 5 GHz. Both are ping RTTs, so one-way
figures are lower, but the tail shape holds.

> **You cannot buffer your way out of the Wi-Fi tail inside 80 ms.** Size the
> buffer to roughly p95 and conceal the tail. Force 5 GHz and disable Wi-Fi
> power save on the Android side.

---

## B. Clock drift

### The arithmetic

Consumer crystals are typically +/-20 to 50 ppm initial, with +/-100 ppm as a
combined tolerance-plus-temperature worst case. **Relative drift is the sum of
both devices' errors**, so two +/-50 ppm parts can differ by 100 ppm. For
contrast, AES11 holds professional gear to 1 ppm (Grade 1) or 10 ppm (Grade 2).

At 48 kHz, 172,800,000 samples pass per hour:

| Drift | samples/hour | ms/hour |
| --- | --- | --- |
| 10 ppm | 1,728 | 36 |
| **50 ppm** | **8,640** | **180** |
| 100 ppm | 17,280 | 360 |

Time to under- or overrun with `H` seconds of headroom is `H / (D * 1e-6)`:

| Headroom | 10 ppm | 50 ppm | 100 ppm |
| --- | --- | --- | --- |
| **30 ms** | 50 min | **10 min** | **5 min** |
| 100 ms | 2.8 h | 33 min | 17 min |

> **At Sonduit's target there is essentially no headroom. Drift correction is
> mandatory; an uncorrected stream glitches within single-digit minutes.**

Holding level requires adding or removing `0.048 * D` frames per second: about
2.4 frames/s at 50 ppm, one frame every 417 ms.

### Detection

Every real implementation uses the **buffer fill level**, because it needs no
clock synchronisation: if you consume at exactly the local hardware rate, the
derivative of the buffer level *is* the drift.

- **Snapcast** keeps three nested medians (20 / 100 / 500 chunks) and requires
  them to agree before acting. Medians rather than means, because they are
  outlier-immune, which matters enormously on Wi-Fi.
- **roc-toolkit** names the signal `niq` and documents the split precisely:
  *"the proportional component counteracts the deviation of the queue length
  observable at the moment, and the integral component counteracts the steady
  clock difference [...] observable over time."* **P handles jitter, I handles
  drift.**
- **PulseAudio** applies its 1-pole filter (`FILTER_PARAMETER 0.125`) **twice**,
  giving a 2-pole lowpass with roughly a 16 s time constant, plus a hard
  outlier rejector that discards any drift estimate above 1% of base rate.

Sonduit's approach — least-squares regression of sender frames against receiver
time — is the timestamp-regression method, and the research independently
confirms the window sizing: *"with ~5 ms of jitter and a 60 s window [...]
comfortably resolvable. A 30-60 s window is enough to measure drift to a few
ppm."* Our 4096-observation window is about 25 s, which is the same order.

**Reset the estimator on route change, suspend, or a large time gap.**
PulseAudio does exactly this; without it a laptop sleep injects a garbage
estimate that takes 30+ seconds to unwind.

### Correction

Audibility of deleting one sample at 48 kHz, computed from `2A*sin(pi*f/48000)`:

| Frequency | Error level vs a full-scale tone |
| --- | --- |
| 100 Hz | -37.7 dBFS |
| 1 kHz | -17.7 dBFS |
| 5 kHz | -3.7 dBFS |
| 10 kHz | above full scale |

So single-sample stuffing is inaudible in quiet or low-frequency passages and a
plain click on loud high-frequency content.

| Option | Artefacts | Good for |
| --- | --- | --- |
| Drop/insert one sample | Level dependent, -38 to 0 dBFS | Emergency, low-level passages, weak hardware |
| Drop a whole packet | Clearly audible | Hard resync only |
| **ASRC with a PI-driven ratio** | **None at ppm scale** | **Steady-state drift** |
| WSOLA splice | Small, content-dependent | Step corrections after a spike |
| OS playback-rate API | Implementation-dependent | Avoid; not portable |

A 100 ppm pitch shift is 0.0017 semitones, which is why ASRC is the right
primary mechanism. roc allows +/-0.5% of ratio authority updated every 5 ms,
with PI gains `P=1e-6, I=5e-9` (gradual) or `I=1e-10` (responsive).

roc's optimisation is worth copying: combine a **static** high-quality
resampler with a **cheap dynamic decimator**, since the dynamic part stays so
close to 1.0 that it only touches 5-20 samples per second.

**Do not use the OS rate API.** Android's `setPlaybackParams` routes through a
time-stretcher; WASAPI shared mode offers no rate control at all.

### Rust resampling

**`rubato`** is the answer. `Async::set_resample_ratio_relative(ratio, ramp)`
does exactly what is needed, ramping in step-size space rather than jumping.
v5.0.0 released 2026-08-10, actively maintained, MIT/Apache-2.0, with Neon SIMD
on aarch64. `max_resample_ratio_relative` must be declared up front; 1.002
gives +/-2000 ppm, ample.

It is **not `no_std`** — verified directly, `src/lib.rs` has no `#![no_std]`
and `asynchro.rs` imports from `std`. Irrelevant for Sonduit.

Fallback if it proves too expensive on low-end Android:
`speexdsp-resampler`, the same algorithm family roc uses. Avoid
`libsamplerate` bindings on licence grounds.

## Sources

- https://www.rfc-editor.org/rfc/rfc3550.txt (6.4.1, 6.4.4, Appendix A.8)
- https://www.cs.columbia.edu/~hgs/papers/Ramj94_Adaptive.pdf
- https://github.com/blueboxd/webrtc/blob/master.lion/modules/audio_coding/neteq/g3doc/index.md
- https://raw.githubusercontent.com/webrtc-mirror/webrtc/main/modules/audio_coding/neteq/delay_manager.h
- https://raw.githubusercontent.com/webrtc-mirror/webrtc/main/modules/audio_coding/neteq/underrun_optimizer.cc
- https://raw.githubusercontent.com/webrtc-mirror/webrtc/main/modules/audio_coding/neteq/time_stretch.cc
- https://roc-streaming.org/toolkit/docs/internals/fe_resampler.html
- https://raw.githubusercontent.com/roc-streaming/roc-toolkit/develop/src/internal_modules/roc_audio/latency_config.h
- https://raw.githubusercontent.com/roc-streaming/roc-toolkit/develop/src/internal_modules/roc_audio/freq_estimator.cpp
- https://raw.githubusercontent.com/pulseaudio/pulseaudio/master/src/modules/module-loopback.c
- https://raw.githubusercontent.com/badaix/snapcast/develop/client/stream.cpp
- https://raw.githubusercontent.com/mikebrady/shairport-sync/master/player.c
- https://docs.pipewire.org/page_man_pipewire-props_7.html
- https://netman.aiops.org/wp-content/uploads/2015/11/mobisys16-sui.pdf
- https://hpbn.co/wifi/
- https://arxiv.org/pdf/2111.09281
- https://www.siward.com/en/about/industry/Frequency_Tolerance_vs_Frequency_Stability__A_Detailed_Look_into_Quartz_Crystal
- https://github.com/HEnquist/rubato
- https://crates.io/crates/rubato

## Not verified

1. **No ppm figure exists for any specific PC sound card or Android SoC audio
   clock.** Only generic crystal ranges and standards figures. Measure it.
2. The Ramjee equations were recovered from a PDF whose Greek characters do not
   survive text extraction; the structure is inferred from the surrounding
   prose. Verify against IEEE Xplore before quoting.
3. The EWMA arithmetic, the J-to-sigma conversion, the drift tables and the
   sample-drop audibility table are **this project's own calculations**, shown
   so they can be checked. The J-to-sigma conversion assumes i.i.d. Gaussian
   delays, which real networks violate.
4. Android `setPlaybackParams` behaviour at ppm scale was asserted, not
   verified against AOSP.
5. roc's PI gains are from the `develop` branch, not a tagged release.
6. The 802.11be figures are simulation, not measurement. The Wi-Fi measurements
   are RTT-based, so one-way jitter is lower.
7. The claim that `setBufferCapacityInFrames` inflates `framesPerBurst` rests
   on one issue report plus a Google engineer confirming intent; not verified
   in AOSP source.
