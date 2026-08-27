# ADR-002: Desktop capture and driver distribution

- Status: accepted, and it **overturns the original plan**
- Date: 2026-08-27
- Supersedes: the assumption that Scream's signed driver can be shipped as-is

## Context

The product requirement is that **Windows shows the Android phone as a
selectable audio output endpoint**. The plan was to vendor Scream's prebuilt,
already-signed driver unmodified, on the theory that MS-PL permits
redistribution and the existing signature carries it.

Research disproved the second half. See
`research/wasapi-vs-virtual-driver.md` for the evidence.

## The finding

`signtool` was run against
`third_party/reference/scream/Install/driver/x64/`:

- The signing certificate **expired on 2023-07-07** and the files are **not
  timestamped**, so the signature does not survive expiry.
- There is **no Microsoft signature at all**; the chain ends at USERTrust.
- **`signtool verify /pa` returns 0x800B0101: zero signatures verified.**
- Since **April 2026**, cross-signed kernel drivers are untrusted by default on
  current Windows 11 and Server 2025.

**An installer shipping these binaries fails on every correctly configured
Windows machine.** This is independent of the licence, which does permit it.

Two further constraints follow:

- **No user-mode API can create a Windows audio endpoint.** PortCls and ACX are
  both kernel-mode; ACX is explicitly KMDF rather than UMDF for latency
  reasons. APOs attach to an endpoint, never create one, and shipping an
  unsigned one requires disabling audio signature verification machine-wide.
- **Any modification voids the catalog**, including editing the INF to change
  the manufacturer from "Tom Kistner". Microsoft is the sole issuer of
  production kernel signatures, an EV certificate is mandatory to even
  register, and Azure Artifact Signing explicitly cannot sign kernel drivers.

## Decision

**Split the goal in two, and be honest about which half is shipping.**

### Tier 1 (planned first): user-mode capture, no endpoint

**Not implemented yet.** `sonduit-capture-win` currently declares the two modes
below and returns `todo!()`; the work is tracked in `roadmap.md`. The decision
recorded here is which two modes it will implement:

- **`ProcessLoopback`** on Windows 11 (build 20348+). Preferred: it is
  endpoint-independent and **emits silence rather than stalling** when nothing
  is playing, which keeps the stream clocked. Its format cannot be negotiated
  (`GetMixFormat` returns `E_NOTIMPL`), so 48 kHz / 16-bit / stereo is
  hardcoded, which suits us.
- **`EndpointLoopback`** on Windows 10, with a **silent event-driven render
  stream** kept open on the same endpoint. That workaround does double duty: it
  stops the documented-but-unreliable loopback event from stalling during
  silence, and it pins the engine period lower than the 10 ms default, which
  cannot be requested from a loopback stream because
  `IAudioClient3::InitializeSharedAudioStream` rejects the loopback flag.

Once written, this gives a working bridge. **It will not satisfy the endpoint
requirement**; audio still plays locally as well.

### Tier 2 (later, gated on money and time): a signed endpoint driver

A virtual audio device of our own, submitted through Partner Center. Blocked on
an EV certificate and, most likely, full WHCP certification. Tracked in
`roadmap.md` as the highest-risk unknown.

### The driver directory

`driver/` stays in the repository layout but **ships nothing yet**. When it
does, the contents are MS-PL and unmodified, with the licence text and
attribution alongside, per `licensing.md`.

## Consequences

- The first shippable Sonduit is a **loopback bridge**, not an output device.
  The README must say so plainly rather than implying otherwise.
- Windows 10 gets the worse path, and Windows 10 is out of support since
  October 2025, so this cost shrinks over time.
- **DSCP marking is unavailable** regardless of tier: the Scream driver parses a
  `DSCP` registry value but the call that would apply it is commented out in
  its source, so no WMM voice queueing can come from it.
- An intermediate worth costing: depend on an already-signed third-party
  virtual cable the user installs, and capture from it by ordinary loopback.
  Not chosen, because it moves a licensing and support problem onto a third
  party without solving it.

## Not verified

Whether an **attestation-signed** (non-WHCP) kernel driver still loads under
the April 2026 enforcement policy. The docs describe attestation as covering
kernel-mode desktop drivers; the policy is written entirely around WHCP. This
is the single most decision-critical unknown in the project and must be
resolved with Microsoft before any driver budget is committed.
