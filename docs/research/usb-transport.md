# USB transport

Researched 2026-08-27. **The single-UDP-code-path theory holds**, but three of
the assumptions underneath it are wrong.

Bandwidth context: 48 kHz / 16-bit / stereo is 1.536 Mbit/s. Every option below
clears that by one to two orders of magnitude. **Throughput is not the deciding
factor; jitter, setup friction and permissions are.**

---

## 1. The definitive answer on adb

> **`adb forward` and `adb reverse` are TCP-only. UDP is not supported, in any
> version, in any form.**

Three independent lines of evidence from AOSP `packages/modules/adb`:

1. The client help text enumerates every accepted spec: `tcp:`,
   `localabstract:`, `localreserved:`, `localfilesystem:`, `dev:`, `dev-raw:`,
   `jdwp:`, `vsock:`, `acceptfd:`. **No `udp:`.**
2. `socket_spec.cpp`, the file that parses all of them, contains **zero**
   occurrences of "udp" and **zero** of `SOCK_DGRAM`. Every socket creation in
   its 476 lines is `SOCK_STREAM`.
3. Architecturally it could not be otherwise: adb multiplexes streams over its
   own reliable, ordered, windowed protocol on a USB bulk pipe. It has no
   datagram-shaped concept.

The only "UDP forwarding" in the Android ecosystem is the **emulator's**
`redir add udp:` console command, which is unrelated and does not apply to
physical devices.

**What TCP-only would cost.** Over a cable the physical layer is effectively
lossless, so the usual head-of-line argument barely applies. The real problem
is buffering: `adb.h` sets `MAX_PAYLOAD = 1 MiB` and
`INITIAL_DELAYED_ACK_BYTES = 32 MiB`. A 32 MiB in-flight window is **about 170
seconds of audio** at our bitrate. If the reader ever falls behind, latency
accumulates without bound and never recovers, because nothing in the stack can
drop a stale frame. You would end up reimplementing UDP semantics on top of a
reliable pipe.

**Consumer viability: none.** Requires USB debugging in Developer Options plus
a per-host RSA prompt. Keep it as a developer transport only.

## 2. USB tethering is the answer

When tethering is enabled, AOSP calls `setCurrentFunctions` with either
`FUNCTION_RNDIS` or `FUNCTION_NCM`. Which one is **not** an Android-version
question: it is a per-device build overlay,
`config_tether_usb_functions`, whose AOSP default is 0 (RNDIS). Treat it as
runtime-detectable, not predictable.

### The Windows driver cliff

| Protocol | Inbox driver | Windows support |
| --- | --- | --- |
| RNDIS | `Rndismp.sys` | **Windows 10 and 11** |
| NCM | `UsbNcm.sys` | **Windows 11 and Server 2022 only** |

An NCM-tethering phone on **Windows 10 has no inbox driver** and appears as an
unrecognised device.

### The addressing assumption is wrong

> **The widely-repeated `192.168.42.129` is obsolete.** Modern AOSP does not
> hardcode a USB tethering address.

From `PrivateAddressCoordinator.java`: only Wi-Fi P2P (192.168.49.1) and
Bluetooth (192.168.44.1) remain hardcoded. USB goes through
`requestDownstreamAddress`, documented as *"Pick a random available address"*,
over the full RFC1918 space. The host portion is randomised, not just a subnet
index, and on **Android 16 and later the pool itself is weighted**: 93.7% of
the time it lands in `10.0.0.0/8`.

The address is sticky per session but **not stable across reboots**, and the
coordinator actively renegotiates on conflict with the phone's upstream.

> **Never hardcode an IP.** Enumerate Windows interfaces, find the RNDIS/NCM
> adapter, and read its DHCP-assigned default gateway — that is the phone.

### UDP works, with four caveats

PC-to-phone traffic addressed to the phone's own tether-interface address is
**local delivery on a directly connected link — no NAT, no forwarding**. NAT
only applies to traffic routed *through* the phone.

1. **Windows Firewall** classifies the new adapter as an unidentified network,
   so it lands on the **Public profile and blocks all inbound**. Outbound
   PC-to-phone is fine; phone-to-PC replies are dropped. The installer must add
   an inbound rule.
2. **Android socket binding.** A wildcard-bound socket receives fine, but
   **replies route out the phone's default network** (Wi-Fi), not back down the
   tether. Bind explicitly to the tether-side local address.
3. **Subnet collision.** The coordinator avoids conflicts with the phone's
   upstream but knows nothing about the PC's other interfaces.
4. **Default-route hijack.** The phone advertises itself as gateway; Windows
   may route general internet traffic over USB. Widely reported. Raise the
   adapter's interface metric.

### Latency

No rigorous citable benchmark exists. An XDA thread reports
`rtt avg 1.179 ms` over USB tethering, but that came from a search summary of a
page that returned HTTP 403 — treat as indicative only. **No published jitter
or packet-loss measurement for RNDIS/NCM exists**, which is the number that
actually matters.

RNDIS and especially NCM **aggregate frames**. Send about 5 ms of audio per
datagram so aggregation timers never hold data.

### Does it need mobile data?

Architecturally no: the downstream is brought up independently of upstream
selection, so an IP link with no internet is the designed behaviour. **But**
`EntitlementManager` lets a carrier gate tethering entirely, and multiple forum
reports describe the toggle being greyed out on some OEM builds without data.
**This is the single biggest risk to the tethering plan and must be tested on
real Samsung, Xiaomi and OPPO builds.**

**Wi-Fi is not disrupted.** Unlike hotspot tethering, USB tethering does not
touch the radio, and AOSP actively prefers a non-cellular upstream. The Wi-Fi
and USB paths can coexist on the same phone.

One real side effect: `setCurrentFunctions` **replaces** the USB function set,
so tethering and AOA cannot both be active. ADB survives.

## 3. USB Accessory (AOA): good permissions, fatal driver problem

The PC becomes host, the phone an accessory, via control requests 51, 52 and
53; the device re-enumerates as VID `0x18D1`, PID `0x2D00` or `0x2D01` with two
bulk endpoints.

**Advantages:** no USB debugging, no root, auto-launch via a
`USB_ACCESSORY_ATTACHED` intent filter, and permission granted along with the
intent.

**The blocker:** the PC side needs libusb, which needs a WinUSB driver bound to
the device. The post-switch `18D1:2D00` half is solvable with a signed WinUSB
INF installed by `pnputil`. **The pre-switch half is not**: requests 51-53 must
be sent to the phone in its *normal* mode, whose VID/PID is OEM-specific and
already bound to MTP/ADB drivers. The reference project's answer is Zadig,
manually, **twice**, which is unacceptable in a consumer installer and breaks
MTP/ADB for that device.

Bulk transfers are also the **lowest-priority** USB transfer type, scheduled
only with leftover bandwidth. The reference project's author flags exactly this
as its known weakness.

## 4. UAC gadget mode: not viable

**It does not exist in AOSP.** `UsbManager.java` enumerates every gadget
function Android knows: ADB, ACCESSORY, MTP, MIDI, PTP, RNDIS, AUDIO_SOURCE,
UVC, NCM. **There is no UAC function**, and the settable subset is narrower
still.

- `FUNCTION_AUDIO_SOURCE` is the AOAv2 path, streams the wrong direction, and
  was **deprecated in Android 8.0**.
- The precedent cuts against it: Google built the whole plumbing for a media
  gadget function in Android 14 (`FUNCTION_UVC`, "use phone as a webcam") and
  **chose not to add UAC.**
- The kernel `f_uac2` module exists but reaching it needs configfs writes that
  are locked down by SELinux on stock devices.

Close this line of investigation.

## 5. Recommendation

**Ship USB tethering over the same UDP code path as Wi-Fi.** It is the only
option that is simultaneously truly UDP, driverless on Windows 11, free of USB
debugging, and near-zero additional code.

The single-code-path theory survives, with these corrections:

| Assumption | Reality | Fix |
| --- | --- | --- |
| Phone is at a known address | False on Android 11+, badly so on 16+ | Read the adapter's DHCP gateway |
| Sending to the phone's IP is enough | Both interfaces may have routes | Bind to the tether adapter's source IP; set `IP_UNICAST_IF` |
| The phone will just receive it | Replies route out Wi-Fi | Bind the Android socket to the tether address |
| Bidirectional UDP works out of the box | Firewall blocks inbound | Installer adds an inbound rule |
| Tethering is one toggle away | Carrier entitlement can veto it | Detect and fall back to Wi-Fi |
| Any phone tethers to any PC | NCM phone + Windows 10 = no driver | Detect and message |

Also: the tether link is essentially lossless, so **make the jitter buffer
depth transport-aware** rather than a single constant. That is where the
25-50 ms USB target is won.

## Sources

- https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/socket_spec.cpp
- https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/client/commandline.cpp
- https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/adb.h
- https://android.googlesource.com/platform/packages/modules/Connectivity/+/refs/heads/main/Tethering/res/values/config.xml
- https://android.googlesource.com/platform/packages/modules/Connectivity/+/refs/heads/main/Tethering/src/com/android/networkstack/tethering/Tethering.java
- https://android.googlesource.com/platform/packages/modules/Connectivity/+/refs/heads/main/staticlibs/device/com/android/net/module/util/PrivateAddressCoordinator.java
- https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/hardware/usb/UsbManager.java
- https://source.android.com/docs/core/interaction/accessories/aoa
- https://source.android.com/docs/core/interaction/accessories/aoa2
- https://source.android.com/docs/core/ota/modular-system/tethering
- https://developer.android.com/develop/connectivity/usb/accessory
- https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/supported-usb-classes
- https://learn.microsoft.com/en-US/troubleshoot/windows-server/networking/automatic-metric-for-ipv4-routes
- https://github.com/microsoft/NCM-Driver-for-Windows
- https://github.com/BreadFish64/AndroidUsbAudioDevice (README only; no licence)
- https://www.kernel.org/doc/html/latest/usb/gadget-testing.html

## Not verified

1. **Which Android version, if any, mandated NCM over RNDIS.** The mechanism
   was found; no AOSP commit or CDD requirement flipping it was.
2. **Which shipping devices actually use NCM.** No confirmed device-to-protocol
   mapping for any current phone. Measure it.
3. **Whether tethering can be enabled with no upstream on real OEM builds.**
   AOSP clearly permits it; the "greyed out" reports are forum-level. **Test
   this; it is the biggest risk to the plan.**
4. **Any controlled latency or jitter benchmark of USB tethering.** The ~1.18 ms
   RTT figure came from a search summary of a page that returned 403.
5. Any AOA bulk throughput or latency benchmark. None found.
6. Any controlled `adb forward` benchmark. Only scattered community numbers.
7. Whether AOSP's accessory gadget emits Microsoft OS descriptors, which would
   auto-bind WinUSB. Worth 30 minutes if AOA is ever pursued.
8. Exact Android behaviour when binding a UDP socket to a tether downstream
   interface. The recommendation rests on general routing behaviour, not on
   documentation or a test. **Verify on-device.**
9. Whether the 32 MiB adb window is the effective one in shipping builds. The
   qualitative point holds regardless.
