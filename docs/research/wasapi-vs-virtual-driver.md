# Windows capture and driver distribution

Researched 2026-08-27.

**This document overturns a core project assumption.** The plan was to ship
Scream's prebuilt signed driver unmodified. That is not viable, and the reason
is not licensing.

---

## 1. The finding: Scream's signed binaries are cryptographically dead

Verified locally with `signtool.exe` from Windows Kits 10.0.26100 against
`third_party/reference/scream/Install/driver/x64/`:

```
Leaf:   CN=Tom Kistner, O=Tom Kistner, C=DE
Issuer: CN=Sectigo RSA Code Signing CA
Root:   USERTrust RSA Certification Authority
Valid:  2020-07-06 -> 2023-07-07   (EXPIRED)
File is not timestamped.
signtool verify /pa  ->  0x800B0101  (0 signatures verified)
signtool verify /kp  ->  0x800B0101  (0 files verified)
```

Same result for `arm64`. Three independent problems, any one of them fatal:

1. **The certificate expired on 2023-07-07 and the files are not timestamped.**
   Microsoft's rule is that a driver keeps working past certificate expiry only
   if it was timestamped before it. These were not.
2. **There is no Microsoft signature at all.** The chain ends at USERTrust, not
   at the Microsoft Code Verification Root. It never satisfied the Windows 10
   1607+ kernel-mode signing policy; Scream's own README tells users to disable
   Secure Boot or enable test signing.
3. **Since April 2026, cross-signed kernel drivers are untrusted by default** on
   Windows 11 24H2/25H2/26H1 and Server 2025.

**Consequence: an installer shipping these binaries fails on every correctly
configured Windows 10 or 11 machine.** This is a technical fact independent of
MS-PL, which does permit the redistribution.

## 2. Signing a driver of our own

- An **EV code-signing certificate is mandatory** simply to register for the
  Hardware Developer Program, before any submission.
- **Microsoft is the sole issuer of production kernel-mode signatures.** From
  the docs: *"all production driver packages must be submitted to, and signed
  by Microsoft"*, and *"every time a Production level driver package is rebuilt,
  Microsoft must sign the package"*.
- **Any modification voids the catalog**, including a one-line INF edit to
  change the manufacturer name from "Tom Kistner".
- Attestation signing does not require HLK testing and does cover kernel-mode
  desktop drivers, but Microsoft's current page is titled *"Attestation signed
  drivers for testing scenarios"* and the April 2026 policy is written
  exclusively around WHCP certification. **Whether an attestation-signed driver
  still loads under the new enforcement is the single most decision-critical
  thing that could not be established.** Plan for full WHCP.
- **Azure Artifact Signing (formerly Trusted Signing) cannot sign kernel
  drivers** and does not issue EV certificates. It is still the right tool for
  Sonduit's user-mode binaries and installer.

Cost and lead time were not established from primary sources and are
deliberately not quoted here.

## 3. The user-mode options, and what they cannot do

**No user-mode API can create a Windows audio endpoint.** PortCls and its
modern successor ACX are both kernel-mode; ACX is explicitly built on KMDF
rather than UMDF to avoid the task-switching latency. This is the hard
constraint on the product requirement that Windows show the phone as a
selectable output device.

### WASAPI endpoint loopback

- Shared-mode only; taps a render endpoint, so audio still plays locally.
- Documented as event-driven since Windows 10 1703, but **the event is driven
  by render activity**: with nothing playing, callbacks stop and blocking reads
  hang indefinitely while the stream still reports active. NAudio's own docs
  recommend playing silence for the duration of a recording.
- **`IAudioClient3::InitializeSharedAudioStream` rejects the loopback flag**
  (`AUDCLNT_E_INVALID_STREAM_FLAG`), confirmed by a Microsoft engineer, so the
  engine period cannot be lowered from the loopback stream itself. It can be
  lowered indirectly by opening a second silent event-driven render stream at a
  small period, which also fixes the silence stall.
- Microsoft publishes **no loopback-specific latency figure**. Structurally it
  is about one engine period, default 10 ms.
- Windows 11 ARM64 has a distinct bug where `GetNextPacketSize` always returns 0.

### WASAPI process loopback

- `ActivateAudioInterfaceAsync` with `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK`.
- **Emits silence rather than stalling** when nothing is playing, which solves
  the clocking problem cleanly.
- Endpoint-independent, so no re-plumbing on default-device change.
- Requires build 20348, which in retail terms means **Windows 11 only**.
- `GetMixFormat` and `IsFormatSupported` return `E_NOTIMPL`; the format must be
  hardcoded.
- **Still does not create an endpoint.**

### APOs

Attach to an existing endpoint, never create one. Shipping an unsigned APO
requires setting `DisableProtectedAudioDG=1` machine-wide, which disables audio
signature verification for every application. Not defensible in a product.

## 4. What Scream actually does

A WDM/PortCls miniport derived from Microsoft's MSVAD sample, using the legacy
**WaveCyclic** port class rather than WaveRT. It emulates DMA with a KTIMER and
DPC, and `GetPosition` **interpolates from the wall clock** rather than
measuring hardware, so its stream position is a computed estimate. Audio is
tapped in `CopyTo()` and pushed to a WSK datagram socket from an IO work item.

The README claims only that delay is "minimal"; **no numeric latency figure is
claimed anywhere in the project.**

## 5. What this means for Sonduit

Recorded as the decision in ADR-002. In short, a two-tier approach:

- **Tier 1, buildable now, zero signing exposure:** process loopback on
  Windows 11, endpoint loopback plus a silent render keepalive on Windows 10.
  This ships a working bridge but does **not** show the phone as an output
  device.
- **Tier 2, the actual product requirement:** a signed virtual endpoint driver.
  Months and money, gated on EV certification and WHCP.

An intermediate worth costing: depend on an already-signed third-party virtual
cable that the user installs, and capture from it by ordinary loopback.

## Sources

- https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording
- https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient3-initializesharedaudiostream
- https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_activation_params
- https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/
- https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio
- https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/acx-audio-class-extensions-overview
- https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-processing-object-architecture
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-reqs
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/hardware-program-register
- https://learn.microsoft.com/en-us/windows-hardware/drivers/install/deprecation-of-software-publisher-certificates-and-commercial-release-certificates
- https://support.microsoft.com/en-us/windows/hardware/drivers/the-windows-driver-policy
- https://learn.microsoft.com/en-us/azure/artifact-signing/faq
- https://learn.microsoft.com/en-us/answers/questions/280278/is-it-possible-to-use-software-loopback-functional
- https://learn.microsoft.com/en-us/answers/questions/1125409/loopbackcapture-(-activateaudiointerfaceasync-with
- https://learn.microsoft.com/en-us/answers/questions/5694431/coreaudio-wasapi-loopback-on-windows-11-arm-iaudio
- https://github.com/PortAudio/portaudio/issues/935
- https://github.com/naudio/NAudio/blob/master/Docs/WasapiLoopbackCapture.md
- https://sourceforge.net/p/equalizerapo/wiki/Documentation/
- Local: `third_party/reference/scream/` and `signtool.exe`

## Not verified

1. **Whether an attestation-signed, non-WHCP kernel driver still loads under
   the April 2026 enforcement policy.** The most important open question here.
   Resolve through Partner Center support before committing budget.
2. Numeric latency added by WASAPI loopback. No published figure exists; the
   "about one engine period" estimate is structural inference.
3. Whether a purely virtual audio device can pass Device.Audio HLK. Shipping
   products suggest yes; that is circumstantial.
4. EV certificate cost, Partner Center fees, and lead times. No primary source
   found; deliberately not estimated.
5. Whether process loopback supports `IAudioClient3` or small engine periods.
6. Whether the x86 Scream binaries share the signature problem. x64 and arm64
   were verified and both failed identically; x86 was not separately checked.
