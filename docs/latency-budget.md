# Latency budget

Targets: **40-80 ms over Wi-Fi**, **25-50 ms over USB**, mouth to ear, at
48 kHz / 16-bit / stereo.

> **Every number in this document is a budget, not a measurement.** Audio has
> since been played on a real phone over USB tethering, which proves the chain
> carries sound; it produced no timing figure. Nothing here has been measured
> end to end, because that needs a loopback cable and a session nobody has run
> yet, and there is still no signed capture driver (`environment.md`). Figures
> are derived from vendor documentation and from arithmetic that is shown so it
> can be checked. **Do not quote any of these as a result.**

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
The current implementation does not do this yet.

### Why not simply make it smaller

Measured Wi-Fi latency has a long tail. A campus WLAN study measured 3 / 20 /
250 ms at the 50th, 90th and 99th percentiles; a home 2.4 GHz measurement saw
6.22 ms median against 34.87 ms at p95, versus 0.90 / 1.58 ms on 5 GHz.

**The p99 Wi-Fi tail does not fit in an 80 ms budget and never will.** The
buffer is sized near p95 and the tail is concealed, not absorbed. Forcing 5 GHz
and disabling Wi-Fi power save on the phone move the distribution more than any
amount of buffering.

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

## 6. How this document is used

**Any pull request that adds latency to a stage must say which stage and how
much, and update this table.** A change that pushes the total past the target
band needs a compensating reduction elsewhere or an explicit decision to move
the target.

When real measurements exist they replace the budget column, and this notice
comes off. Until then the honest summary is:

> Sonduit's latency has never been measured. These are the numbers the design
> is aiming at.

## Sources

Full citations in `research/`:

- Windows engine latency: `research/wasapi-vs-virtual-driver.md`
- Android burst sizes, granted-mode behaviour and round-trip figures:
  `research/android-aaudio.md`
- Wi-Fi and USB delay distributions: `research/jitter-and-drift.md`,
  `research/usb-transport.md`
- Packet arithmetic: `protocol.md`
