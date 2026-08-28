# ADR-004: One transport for Wi-Fi and USB

- Status: accepted, assumption **confirmed** with corrections; the firewall
  consequence was **amended on 2026-08-28**
- Date: 2026-08-27
- Amended: 2026-08-28, see [Windows Firewall blocks the return path](#consequences-and-the-assumptions-that-were-wrong)

## Context

Sonduit needs both a Wi-Fi path (40-80 ms target) and a USB path (25-50 ms).
The plan was a single raw UDP code path for both, on the theory that USB
tethering presents as an ordinary IP interface so the two differ only in
routing.

## Decision

**One UDP implementation. Wi-Fi and USB differ only in which local address the
socket binds to.** There is deliberately no second transport.

Four options were evaluated (`research/usb-transport.md`):

| Option | Verdict |
| --- | --- |
| **USB tethering (RNDIS/NCM)** | **Chosen.** Real IP, real UDP, driverless on Windows 11, no developer mode |
| `adb forward` / `reverse` | **TCP-only, verified in AOSP source.** Developer transport only |
| USB Accessory (AOA) | Best permissions, but the Windows driver problem is unsolvable generically |
| UAC gadget mode | **Does not exist in AOSP.** Closed |

### adb is TCP-only, definitively

`socket_spec.cpp` contains zero occurrences of "udp" and zero of `SOCK_DGRAM`;
every socket it creates is `SOCK_STREAM`. The accepted specs are `tcp:`,
`localabstract:`, `localreserved:`, `localfilesystem:`, `dev:`, `jdwp:`,
`vsock:` and `acceptfd:`.

Even ignoring that, adb's 32 MiB in-flight window is **about 170 seconds of
audio** at our bitrate, with no mechanism anywhere in the stack to drop a stale
frame. Latency would accumulate without bound and never recover.

## Consequences, and the assumptions that were wrong

The single-path theory holds. Three assumptions underneath it do not:

1. **The phone is not at a predictable address.** The `192.168.42.129` figure
   is obsolete. Modern AOSP picks a random `/24` from RFC1918 space, and on
   Android 16 and later it lands in `10.0.0.0/8` about 93.7% of the time.
   **Never hardcode an IP**; enumerate the Windows adapters, find the RNDIS or
   NCM one, and read its DHCP gateway. This is why `discovery.rs` is scoped to
   a single interface.

2. **Binding matters on both ends.** With Wi-Fi and USB both up, the PC may
   route out the wrong interface, and on the phone a wildcard-bound socket
   *receives* fine but **replies out the default network**. Both sides must
   bind explicitly to the tether-side address. `bind_sender` takes a local
   address for exactly this reason.

3. **Windows Firewall blocks the return path.** The tether adapter lands on the
   Public profile, which blocks all inbound connections.

   **Amended 2026-08-28.** As originally decided: *the installer must add a
   rule.* As amended, and as it stands: **the installer adds nothing, and
   Windows asks the user once on first run.**

   There was an NSIS `NSIS_HOOK_POSTINSTALL` in
   `desktop/src-tauri/installer/hooks.nsh` running
   `netsh advfirewall firewall add rule ... protocol=UDP localport=4011`. The
   hook file is deleted and so is the `bundle.windows.nsis.installerHooks`
   entry that pointed at it, because it could not work and could not have
   worked:

   - The NSIS installer is per user (`CurrentUser`), so it has no
     administrator rights and `netsh` fails. The hook noticed, printed the
     failure code into the install log, and carried on.
   - The rule was scoped to a **port**. Windows prompts per **program**, so
     even with the rights it would not have suppressed the prompt it existed
     to suppress.

   Suppressing the prompt for real means a per-machine install, which means a
   UAC prompt on the install and on every update after it. That was weighed
   against one allow-once dialog the first time the app runs, and declined:
   a program that listens on a socket is expected to raise that dialog, and
   the user answers it once. **The absence of firewall handling here is the
   decision, not an omission.**

Additional consequences:

- **NCM phones do not work on Windows 10**, which has no inbox driver for it.
  RNDIS works on both. Which one a phone uses is a per-device build overlay,
  not an Android version, so it must be detected at runtime.
- **Tethering and AOA are mutually exclusive**, because `setCurrentFunctions`
  replaces the USB function set. ADB survives.
- **Wi-Fi is not disrupted by USB tethering**, so both paths can be live at
  once, which makes failover possible.
- **RNDIS and especially NCM aggregate frames.** Sending 5-6 ms of audio per
  datagram keeps aggregation timers from holding data. The Scream payload of
  1152 bytes is 6 ms at 48 kHz / 16-bit / stereo, which happens to be right.
- The tether link is essentially lossless, so **jitter buffer depth must be
  transport-aware**, not a single constant. That is where the 25-50 ms USB
  target is actually won, and the current buffer does not do it yet.

## Risks

**Carrier entitlement can veto tethering entirely**, and forum reports describe
the toggle being greyed out on some OEM builds without mobile data. AOSP
clearly permits a downstream with no upstream, but this is the biggest risk to
the plan and must be tested on real Samsung, Xiaomi and OPPO hardware.
