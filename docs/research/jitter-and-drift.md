# Jitter buffering and clock drift

Researched 2026-08-27. This one **confirms** the project's assumptions and
supplies the constants.

Two corrections have been added to section A since. The first, 2026-09-04, is a
re-reading of roc plus a simulation; the second, later the same day, is a
measurement on the access point the complaint came from, and it contradicts
parts of the first. Both are kept, in order, so the superseded reading stays
legible.

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

#### Correction, 2026-09-04: the 1.7 was read without its other half

The paragraph above is accurate and incomplete, and the incompleteness cost a
defect. `latency_tuner.cpp` was re-read against the complaint that Wi-Fi
latency climbs and does not come back.

**The threshold and the step are one mechanism.** roc does not set the target
to the estimate. It moves it by a fraction derived from the same constant:

```c
lat_update_dec_step_(upper_coef_to_step_lat_update(
    latency_config.latency_decrease_relative_threshold))
// upper_coef_to_step_lat_update(x) = (x + 1.f) / (x * 2.f)
```

At 1.7 that is 0.794, so a decrease takes 21% off the current target and the
next one is 5 s later. A target far above the estimate walks down to it over
several cooldowns; it never arrives in one move. The increase is the mirror,
`(x + 1) / 2` = 1.35, applied to the estimate rather than to the target, so a
growth deliberately overshoots what was measured.

Sonduit transplanted the 1.7 and set the target straight to the estimate on
both sides. With no step, 1.7 stops being hysteresis and becomes a switch:
below the ratio nothing happens at all, above it the whole difference is
applied at once.

**roc's minimum is a clamp on the target, not a floor under the estimate.**
`min_target_latency` bounds the result of the tuner. Sonduit applied its
equivalent, `JitterConfig::target_ms`, inside the estimate, which made the
ratio test compare the held target against a number that could not go below
30 ms on Wi-Fi. No target under 51 ms could satisfy it, and 51 ms is above
anything the estimator produces on an ordinary radio, so the target could grow
and could not shrink. Driven over five minutes of jittery arrivals the grow
counter reached eighteen and the shrink counter stayed at zero, in every
pattern tried.

> Measured later the same day, the last sentence is wrong about the real link:
> the shipped build shrank three times in fifteen minutes on the reporter's
> access point. See *Measured on the access point*, below.

**The peak term was not transplanted either, and its absence is visible.**
roc's estimate is `MAX(peak * 1.2, mean * 3.0)`; Sonduit has only the mean
term. RFC 3550's estimator is a mean absolute *first difference*, and a
station that loses the medium for 120 ms and is then handed its backlog
produces one large difference followed by a run of small ones. Over a 300 s
timeline with a 120 ms stall every 12 s the estimate peaked at 9.3 ms and the
target never left 34 ms. (On the real access point the estimate runs 3.2 to
6.4 ms, median 3.9, so the generated timeline is about twice as hostile as the
radio it stood in for.) **The buffer is close to blind to the failure Wi-Fi
actually has.** Growing it would not currently help -- see the note on the
hand-off below -- so this is recorded rather than acted on.

`crates/sonduit-core/examples/jitter_timeline.rs` is the harness those figures
come from. It drives the shipped `JitterBuffer` through the same calls
`sonduit-ffi`'s receive loop makes, over an arrival timeline it generates from
the distributions in this section, and prints the estimate, the suggestion, the
held target, both counters and what the receiver holds.

#### What depth is worth on a stalling link: nothing, past the hand-off ring

Measured on that harness, and the reason the Wi-Fi ceiling moved from 200 ms to
80 in `JitterConfig::for_transport`.

Audio reaches the callback through `crate::handoff`, and the receive thread
refills that ring **on arrival**. During a stall nothing arrives, so nothing is
handed over, and the callback drains the ring -- at most five packets, 30 ms --
and then plays silence, however much the jitter buffer is holding behind it.

Underrun was therefore identical at ceilings of 200, 120, 80 and 60 ms, to the
millisecond, in every arrival pattern tried:

| Ceiling | Held, median | Held, p95 | Underrun over 300 s |
| --- | --- | --- | --- |
| 200 ms | 122 ms | 194 ms | 2496 ms |
| 120 ms | 90 ms | 121 ms | 2496 ms |
| **80 ms** | **67 ms** | **83 ms** | **2496 ms** |
| 60 ms | 59 ms | 65 ms | 2496 ms |

120 ms stalls every 12 s, backlog released at 2 ms a packet, three seeds, all
agreeing. **145 ms of latency bought nothing.** Depth past the ring is not a
buffer against anything; it is only late audio waiting its turn.

> **Nothing in that table is a link measurement, and two of its columns cannot
> be reproduced on hardware.** The "held" figures describe the harness on a
> generated timeline whose backlog release, 2 ms a packet, is 125 times slower
> than the access point that has since been measured; the real link held 30 ms
> at p50 with the 200 ms ceiling in place. The underrun column cannot be
> checked at all: no build reports a frame count for it. See *Measured on the
> access point*, below.

#### Measured on the access point, 2026-09-04, later the same day

The correction above rests on two things: a reading of roc's source, which is
unaffected by anything here, and a simulation, which stood in for an access
point that could not be reached. It can be reached now. The cable is in **for
adb only, with tethering deliberately off**, so the audio stayed on Wi-Fi and
the radio under test is the one the complaint came from.

Three runs of 300 s on the build before the change (`max_ms` 200) and five runs
after it (`max_ms` 80). The first 60 s of each is discarded. Figures are sampled
from the phone's own log line and from the sender's telemetry snapshot.

| | before, `max_ms` 200 | after, `max_ms` 80 |
| --- | --- | --- |
| jitter-buffer depth, p50 / p95 / max | 30 / 60 / 78 ms | 30 / 36 / 84 ms |
| depth + hand-off queue, p50 / p95 | 48 / 80 ms | 48 / 56 ms |
| sender end-to-end, p50 / p95 / max | 70.8 / 100.6 / 123.7 ms | 67.8 / 79.2 / 166.4 ms |
| target moves downward | 3 in 15 min | 48, against 44 upward |
| median held target | 35 ms | 31 ms |
| receiver loss, p50 | 0.02% | 0.01% |

Per-run p95 depth was 66 / 36 / 66 before and 36 / 72 / 36 / 36 / 36 after.
**The p95 depth difference between the builds is inside the run-to-run spread**
and must not be quoted as a depth reduction.

Two figures moved the wrong way and are not explained here: the maximum
end-to-end reading rose from 123.7 to 166.4 ms and the maximum depth from 78 to
84 ms. Five runs have more chances to produce an extreme reading than three, but
that is a reason to distrust the comparison, not an account of it.

##### What the earlier correction gets wrong about this link

1. **"122 ms median, 194 ms p95 with a 200 ms ceiling" does not describe this
   access point.** Those are the harness's numbers on a generated timeline. On
   the radio, the 200 ms build held p50 30 ms, p95 60 ms and a maximum of 78 ms,
   and never came within 120 ms of its ceiling. **200 was never the operating
   point here.** The table under *What depth is worth on a stalling link* stands
   as a description of the harness and of nothing else.
2. **The mechanism the change was reasoned from is not what this radio does.**
   "An access point releasing its queue slower than four times real time leaves
   the surplus as permanent depth" was the argument; measurement says the
   premise is absent. An aarch64 probe timestamping every datagram with
   `CLOCK_MONOTONIC`, 60 bursts of 32 packets, run twice an hour apart, puts the
   release of a backlog at **p50 0.016 ms per packet** -- about 375 times real
   time, and roughly 90 times on the safe side of the 1.5 ms gap
   `WIRE_SPEED_RATIO` tests for. The wire-speed test fires and a burst never
   becomes depth. The AP does hold packets for tens of milliseconds: under a
   paced 6 ms stream the arrival gaps are p50 5.89 / p95 9.75 / p99 18.7 ms,
   with relative one-way delay at p99 of 24 to 62 ms. It just empties at wire
   speed when it lets go. **Its failure is delay, not backlog.**
3. **"`target_shrank` read zero in every run" and "structurally zero" are false
   of this link.** The old build shrank three times in fifteen minutes, from
   held targets of 66 and 69 ms -- both above the 51 ms the old floor demanded,
   so the estimator does reach past it on a real radio. The defect is real, and
   it is **rarity and violence**: a single 36 ms jump rather than a walk, three
   times in a quarter of an hour. It is not impossibility.
4. **"The largest target the estimator produces on a bad radio is about 50 ms"
   is wrong,** and it was half the argument for choosing 80. The new build
   reached exactly 80 and was clamped there. The ceiling is live, not inert: it
   was reached once in 25 minutes.

**RFC 3550 estimate on this link: 3.2 to 6.4 ms, median 3.9.** The simulation
produced 9.3.

##### What the measurement supports

- **The proportional shrink works and is visible.** One run recorded a target
  walking `80 -> 64 -> 50 -> 40 -> 32 -> 30`, which is the roc step this
  project had transplanted without.
- **The target now tracks continuously**, in a 30 to 35 ms band, with 48
  downward moves against 44 upward, instead of three violent jumps; the median
  held target is 4 ms lower.
- **Sender end-to-end p95 improved from 100.6 ms to 79.2 ms**, which is the
  figure on the maintainer's screen.

Those three are the justification for the change. The simulated one -- that
200 ms was the depth the link settled at -- is withdrawn.

##### What underrun did is still unknown

Nothing above measures underrun, and nothing on the phone can.
`PlaybackCounters::frames_underrun` is incremented in
`crates/sonduit-playback-android/src/aaudio.rs` and **read by nothing**: not the
FFI telemetry, not the log line, not the phone's UI. Every underrun claim in
this project, "underrun is unchanged" included, is either a harness figure or a
reading of `depth 0 + queued 0` in a log line printed every 40 packets, which is
240 ms. That is a proxy for an empty queue sampled four times a second, not a
frame count.

##### What this measurement is not

- One access point, one phone, one machine, one afternoon.
- Not paired: three runs before against five after, on different builds at
  different times, not interleaved.
- Sampled from the phone's log at 240 ms and from a sender snapshot, so both
  ends are the software's own account of itself. No loopback cable is involved
  and nothing here times the analogue path.
- Silent on stalls of the kind the harness models. Nothing in these eight runs
  produced a 120 ms medium loss, so this measurement neither confirms nor
  refutes what the buffer would do in one.

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
- https://raw.githubusercontent.com/roc-streaming/roc-toolkit/develop/src/internal_modules/roc_audio/latency_tuner.cpp
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
8. **The 2026-09-04 correction is a reading of roc's `develop` branch and a
   run of this project's own code against a generated timeline.** No packet
   arrival on the reporter's access point was captured, and none of the delay
   figures driving the harness came from it: the phone could not be reached
   from this machine over Wi-Fi and there was no cable. The buffer's response
   to those inputs is measured; the inputs are assumed.

   **Superseded the same day.** The link has since been measured, in *Measured
   on the access point* above, and four of that correction's claims do not hold
   on it. The roc reading is unaffected; the delay model behind the harness is
   the part that was wrong.
9. **The eight measured runs are the only hardware evidence in this file**, and
   they are one AP, one phone, unpaired before-and-after, sampled at 240 ms
   from the receiver's own log. They do not cover a stall, and they do not
   cover underrun, which no build can currently report.
