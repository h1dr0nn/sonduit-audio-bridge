# Latency budget

Targets: **40-80 ms over Wi-Fi**, **25-50 ms over USB**, mouth to ear, at
48 kHz / 16-bit / stereo.

> **Every number in the budget table is a budget, not a measurement.** Those
> figures are derived from vendor documentation and from arithmetic that is
> shown so it can be checked, and **none of them may be quoted as a result.**
> Nothing in the chain has been measured against a loopback cable, and there is
> still no signed capture driver (`environment.md`).
>
> Two lots of measured content have since been read off running sessions. Both
> are in [section 6](#6-the-sessions-that-have-been-read), on their own and
> outside the table, so that neither can be mistaken for a budget line. One is a
> USB session read off the telemetry panel; the other is eight Wi-Fi runs read
> off the phone's log and the sender's snapshot. Neither confirms the table:
> between them they cover seven of the nine stages, one of them never settled,
> and they found a buffer this document does not list at all.

---

## 1. The chain

```text
  application audio
        |
  [1] Windows audio engine
        |
  [2] capture into Sonduit
        |
  [3] packetisation
        |
  [4] encode and send
        |
  [5] network
        |
  [6] receive and decode
        |
  [7] jitter buffer          <- the only knob that is really ours
        |
  [8] AAudio buffer
        |
  [9] device output path
        |
     speaker
```

## 2. Budget

| # | Stage | Wi-Fi target | USB target | Basis |
| --- | --- | --- | --- | --- |
| 1 | Windows audio engine | 3.0 | 3.0 | Engine processing is documented at 1.3 ms; the rest is one small engine period. Default period is 10 ms and must be lowered deliberately (ADR-002) |
| 2 | Capture into Sonduit | 3.0 | 3.0 | One engine period of hand-off. **No published loopback figure exists** |
| 3 | Packetisation | 6.0 | 6.0 | **Hard floor.** 1152 bytes is 288 frames is exactly 6.000 ms. Arithmetic, not an estimate |
| 4 | Encode and send | 0.5 | 0.5 | Header write and one `sendto`. Encoding is a memcpy |
| 5 | Network | 5.0 | 1.0 | Wi-Fi: ~1 ms median on uncongested 5 GHz, budgeted at p95. USB: sub-millisecond, dominated by the 1 ms USB frame |
| 6 | Receive and decode | 0.5 | 0.5 | One `recvfrom`, a header parse and a copy |
| 7 | **Jitter buffer** | **30.0** | **12.0** | The adaptive target. See section 3 |
| 8 | AAudio buffer | 8.0 | 8.0 | Two bursts. Burst is device-set; 4 ms is a good device, 20 ms is a bad one |
| 9 | Device output path | 8.0 | 8.0 | HAL, DSP and analogue. Not separately documented anywhere |
| | **Total** | **64.0 ms** | **42.0 ms** | |

Both land inside their target bands, with 16 ms of headroom on Wi-Fi and 8 ms
on USB.

## 3. The jitter buffer is where the budget is won or lost

Stages 1, 2, 8 and 9 are set by the two operating systems. Stage 3 is fixed by
the wire format. Stage 5 is the network. **Stage 7 is the only one Sonduit
fully controls**, and it is 47% of the Wi-Fi budget and 29% of the USB budget.

This is why ADR-004's conclusion that **buffer depth must be transport-aware**
matters: a single constant either wastes 18 ms on USB or underruns on Wi-Fi.
`JitterConfig::for_transport` does this.

### Why not simply make it smaller

Measured Wi-Fi latency has a long tail. A campus WLAN study measured 3 / 20 /
250 ms at the 50th, 90th and 99th percentiles; a home 2.4 GHz measurement saw
6.22 ms median against 34.87 ms at p95, versus 0.90 / 1.58 ms on 5 GHz.

**The p99 Wi-Fi tail does not fit in an 80 ms budget and never will.** The
buffer is sized near p95 and the tail is concealed, not absorbed. Forcing 5 GHz
and disabling Wi-Fi power save on the phone move the distribution more than any
amount of buffering.

### What the receiver may hold at all

The 30 ms above is the depth the adaptation aims for. What bounds the depth it
can actually reach is `JitterConfig::max_ms`, applied by `shed_over_budget` to
the jitter buffer **and** the hand-off ring together, because the listener
waits through the sum.

That bound is **80 ms on Wi-Fi and 80 ms on USB**. The Wi-Fi figure was 200 ms
until 2026-09-04, which is two and a half times the whole band this document
targets. It was cut on a simulation which said that on a stalling link 200 ms
was where the depth settled rather than a limit it approached, and that
underrun was identical at every ceiling from 200 ms down to 60.

**Measurement on a real access point later that day contradicts the first of
those and cannot check the second.** On that radio, with the 200 ms ceiling in
place, the depth held a median of 30 ms and a p95 of 60 ms and never came within
120 ms of the ceiling: 200 was not the operating point there. Underrun cannot be
measured on hardware at all, because no build reports a frame count for it. What
the cut did buy, measured, is a target that moves continuously instead of in
rare jumps, and a sender-side p95 that fell from 100.6 ms to 79.2 ms. The
figures are in [section 6](#the-eight-wi-fi-runs-2026-09-04); see also
[ADR-004](adr/ADR-004-transport.md) and `research/jitter-and-drift.md`.

**No line in the table above moves.** This is a bound on the worst case, not
on the budgeted case, and it only ever lowers what stage 7 can reach.

## 4. Where the numbers are weakest

Honestly ranked by how much they could move:

1. **Stage 9, device output path, 8 ms.** Every published Android figure is
   *round-trip*; no output-only table exists for exclusive mode. A Google
   engineer's informal estimate for a FAST track output is 15-40 ms, and about
   120 ms when off it. **If the real figure is at the top of that range the
   Wi-Fi budget fails.**
2. **Stage 8, AAudio buffer, 8 ms.** Two bursts, but the burst is device-set.
   Reported values run from 96 frames (2 ms) to 960 frames (20 ms). On a
   MediaTek Helio device this stage alone is 40 ms and the budget is
   unreachable.
3. **Stage 2, capture, 3 ms.** Microsoft publishes no loopback-specific
   latency. This is structural inference.
4. **Stage 5, network.** Budgeted at roughly p95. Any congestion breaks it.

## 5. What has actually been established

- **Stage 3 is exact.** 1152 bytes / (2 channels * 2 bytes) = 288 frames;
  288 / 48000 = 6.000 ms. Asserted by a test in `sonduit-core`.
- **Stages 4 and 6 are conservative.** Both are a copy and a syscall.
- **Stage 7 is a policy, not a measurement**, and the code reports its actual
  depth and target at runtime through `Telemetry`.
- **Two sets of readings have been taken from running sessions**, one over USB
  and one over Wi-Fi. They are in section 6 rather than in this list, because a
  reading of the software's own account of itself is weaker evidence than the
  arithmetic above and must not be filed beside it.

## 6. The sessions that have been read

**This section and section 7 are the only measured content in this document.**
Nothing in either feeds the table above, and nothing in the table above came
from them. Two sessions are recorded here: a USB one read off the telemetry
panel, and eight Wi-Fi runs read off the phone's log.

### The USB session, 28 August 2026

On 28 August 2026, between 13:00 and 13:05 local time, a desktop build bridged
WASAPI loopback capture to a Google Pixel 7a at 10.10.22.160:4010 over USB
tethering, at 48 kHz / 16-bit / stereo. The person listening judged the audio
good. Seventeen readings were taken from the live Telemetry card without
touching the session, and the phone's own log was read alongside them.

| Local time | Latency | Jitter buffer depth | Packet loss |
| --- | --- | --- | --- |
| 13:00:59 | 40 ms | 12 ms | 0.00% |
| 13:01:19 | 40 ms | 18 ms | 0.00% |
| 13:01:50 | 51 ms | 30 ms | 0.00% |
| 13:02:48 | 60 ms | 36 ms | 0.00% |
| 13:03:25 | 60 ms | 36 ms | 0.00% |
| 13:03:49 | 63 ms | 42 ms | 0.00% |
| 13:04:01 | 69 ms | 42 ms | 0.00% |
| 13:04:57 | 65 ms | 42 ms | 0.00% |

Those rows are a selection. The full seventeen ran between 40 and 69 ms, and
loss was 0.00% across roughly forty thousand packets, with nothing lost at any
point in the window.

Three things follow, and none of them is a validation of the table.

1. **It did not settle.** The figure was 40 ms when it was first read and 69 ms
   four minutes later, and the depth behind it went 12, 18, 24, 30, 36, 42 ms
   and never once shrank. A number still climbing when the observer stops
   watching is not an operating point, and quoting its lowest reading would be
   dishonest.

   *Added 2026-09-04.* "Never once shrank" was literally true of the code as
   well as of this session. The shrink rule compared the held target against a
   suggestion the configuration had already floored, so on Wi-Fi no target
   below 51 ms could satisfy it.

   *Corrected later the same day, on measurement.* The second half of that note
   said no target the estimator produces on either link gets above 51 ms, and
   that is wrong. On the Wi-Fi runs below, the build carrying the bug reached
   held targets of 66 and 69 ms and shrank three times in fifteen minutes. The
   defect was rarity and a 36 ms jump, not impossibility. This USB session
   remains the only direct observation of a target that only climbed.
2. **The panel's figure is stages 1 to 7 and nothing else.** It is the sender's
   own 16 ms, plus a halved round trip, plus the depth the receiver reports.
   Stages 8 and 9 are not in it. Neither is the buffer described below.
3. **Stage 7 ran at three times its USB budget.** The receiver's adaptive
   target settled at 36 ms against the 12 ms budgeted here, and its depth
   reached 42 ms.

Clock drift over the same window ran between -135 and +38 ppm, with the
resampler correcting between 94 and 264 ppm.

### The stage this table does not have

The phone's log reports a second buffer, sitting between the jitter buffer and
the audio callback, that no telemetry carries and that has no line in section 2.
`Handoff::queued_ms` in `sonduit-core` is the single-producer ring the receive
thread pushes into and the callback drains. `JitterBuffer::depth_ms`, the only
depth the feedback report sends back, is measured upstream of it.

Across 480 log lines it held a median of **110 ms**, ranging from 86 to 116 ms,
and it stayed steady there while the depth ahead of it climbed. It never
triggered `resync_if_hopeless`, which fires at four times target: 110 ms sits
under 144 ms and simply persists.

Audio has to cross it. Capture to ear in this session was therefore not 40 to
69 ms. It was that plus roughly 110 ms, before stages 8 and 9 are counted at
all. **The panel understates the path it describes by more than the entire USB
budget.** That is an accounting defect rather than a slow link, and it is the
most useful thing this session produced.

### What the USB reading is not

- One session, one phone, one machine, one cable. Not a benchmark.
- Read off a telemetry panel, not against a loopback cable. Every figure in it
  is the software's own account of itself.
- Judged good by ear. That is a listener's opinion, and an opinion is not
  timing.
- Not Wi-Fi, not a second device, and not an average of anything: a series of
  instantaneous readings, four minutes long.
- Taken from a debug build whose binary was three minutes older than the
  working-tree state of the packetiser and the sender's bridge, so it cannot be
  credited to any particular revision of those.
- Silent on jitter, late packets and reordering. The feedback protocol does not
  carry them and `TelemetryView` sets all three to `None` deliberately, so the
  panel shows nothing for them and this session measured nothing for them.

### The eight Wi-Fi runs, 2026-09-04

**Measured content, and not a budget line.** No figure below belongs in the
table in section 2, and nothing in that table came from these runs.

On 4 September 2026 the cable went in **for adb only, with tethering
deliberately off**, so the audio stayed on Wi-Fi and the access point under test
is the one the latency complaint came from. Three runs of 300 s on the build
with `max_ms` 200, five on the build with `max_ms` 80, the first 60 s of each
discarded, sampled from the phone's own log line and from the sender's telemetry
snapshot.

| | before, `max_ms` 200 | after, `max_ms` 80 |
| --- | --- | --- |
| Jitter buffer depth, p50 / p95 / max | 30 / 60 / 78 ms | 30 / 36 / 84 ms |
| Depth + hand-off queue, p50 / p95 | 48 / 80 ms | 48 / 56 ms |
| Sender end-to-end, p50 / p95 / max | 70.8 / 100.6 / 123.7 ms | 67.8 / 79.2 / 166.4 ms |
| Target moves downward | 3 in 15 min | 48, against 44 upward |
| Median held target | 35 ms | 31 ms |
| Receiver packet loss, p50 | 0.02% | 0.01% |

Per-run p95 depth was 66 / 36 / 66 before and 36 / 72 / 36 / 36 / 36 after.
**The p95 depth difference between the builds is inside the run-to-run spread**
and is not a result. The maximum end-to-end reading and the maximum depth both
moved the wrong way, 123.7 to 166.4 ms and 78 to 84 ms, and are recorded rather
than explained.

How this compares with what the table predicts:

- **Stage 7 is close to its budget on this link.** The table budgets 30 ms of
  jitter buffer on Wi-Fi. The measured depth is a median of 30 ms in both
  builds, with a p95 of 36 to 60 ms. This is the one stage the table gets right
  so far, and it gets it right on the depth rather than on the total.
- **The stage the table still does not have costs 18 ms here.** Depth plus
  hand-off queue is 48 ms at p50 against a depth of 30, so the ring described
  under *The stage this table does not have* held about 18 ms. That is far less
  than the 110 ms the USB session found, but it is not zero and it has no line.
- **The end-to-end figure is not comparable with the 64 ms total.** 70.8 ms at
  p50 is the sender's own account of stages 1 to 7, the same accounting as the
  panel in the USB session: it excludes stages 8 and 9 entirely. Against the
  table's stages 1 to 7, which sum to 48 ms, the measured p50 is 22.8 ms
  heavier, and at p95 the 200 ms build was 52.6 ms heavier.
- **Nothing here measures stages 8 or 9**, which are 16 ms of the budget and
  the two weakest lines in section 4. The gap above is therefore a floor on the
  disagreement, not the disagreement.
- **Loss is not the problem.** 0.01-0.02% at p50 across eight runs on a radio.

**Underrun is not in this table and could not have been.**
`PlaybackCounters::frames_underrun` is incremented on the phone and read by
nothing, so every underrun claim in this project rests either on a harness or on
reading `depth 0 + queued 0` out of a log line printed every 240 ms. That is a
proxy for an empty queue sampled four times a second, not a frame count.

#### What these runs are not

- One access point, one phone, one afternoon.
- Not paired: three runs against five, on different builds at different times,
  not interleaved.
- The software's own account of itself at both ends. No loopback cable, no
  external timing, nothing measuring the analogue path.
- Silent on stalls. No run lost the medium for the sort of interval the
  simulation in `research/jitter-and-drift.md` models, so these runs neither
  confirm nor refute what the buffer does in one.

## 7. What encryption costs, measured

**This is measured content, not a budget.** It is in a section of its own for
the same reason section 6 is: nothing in the table above came from it, and it
must not be read as a line of that table.

Encryption ([ADR-009](adr/ADR-009-audio-encryption.md)) adds work to two
stages, and to no others:

| Stage | Added | As a fraction of the stage's budget |
| --- | --- | --- |
| 3, packetisation, on the sender's capture thread | **+0.002 ms** | 0.03% of 6.0 ms |
| 6, receive and decode, on the receiver's receive thread | **+0.002 ms** | 0.5% of 0.5 ms |

Both round to zero at the resolution this table uses, so **no budget line
moves**. The numbers are written down anyway because CONTRIBUTING asks a change
that adds latency to name the stage and the amount, and this is both.

Measured by `cargo run --release --example seal_cost -p sonduit-transport` on
an i5-12400, 65 536 packets per figure, 1152-byte payloads, five runs:

| What was timed | Per packet |
| --- | --- |
| `Packetizer::push`, cleartext, for scale | 0.025 - 0.037 us |
| **`Packetizer::push`, sealed** -- the sender's call site | **2.00 - 2.08 us** |
| Decode and own the PCM, cleartext, for scale | 0.046 - 0.057 us |
| **Open and own the PCM, sealed** -- the receiver's call site | **2.21 - 2.32 us** |

Those two bold rows are the whole cost at the place the application actually
calls the cipher, not the cipher measured on its own. That distinction is the
point of listing them: the mechanism costs 2.00 - 2.11 us to seal and
2.15 - 2.26 us to open when driven directly, and the call sites land inside
that spread. Wiring it up added nothing measurable, because the sender seals
into the datagram buffer it was going to write anyway and the receiver opens
into a buffer it reuses. **Neither path allocates per packet**, which is the
constraint that made it free.

An ARM core with no SIMD path will be slower. Forcing the portable software
backends on this machine, which is the closest proxy available here, gives
2.21 us and 2.33 us, so the figure is not resting on AVX2 -- and even ten times
the measured cost would be 0.35% of the 6 ms a packet lasts.

## 8. How this document is used

**Any pull request that adds latency to a stage must say which stage and how
much, and update this table.** A change that pushes the total past the target
band needs a compensating reduction elsewhere or an explicit decision to move
the target.

When real measurements exist they replace the budget column, and this notice
comes off. Section 6 is not that: between them the two sessions cover seven of
the nine stages, one of them never settled, they measure the software against
itself rather than against a cable, and they found a buffer this table does not
list. The honest summary is still:

> These are the numbers the design is aiming at, not numbers it has hit. A USB
> session and eight Wi-Fi runs have been read, in section 6, and neither agrees
> with them.

## Sources

Full citations in `research/`:

- Windows engine latency: `research/wasapi-vs-virtual-driver.md`
- Android burst sizes, granted-mode behaviour and round-trip figures:
  `research/android-aaudio.md`
- Wi-Fi and USB delay distributions: `research/jitter-and-drift.md`,
  `research/usb-transport.md`
- Packet arithmetic: `protocol.md`
