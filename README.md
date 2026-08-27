# Sonduit

Low latency system audio bridge from Windows to Android.

Sonduit sends what your PC is playing to a phone on the same network, over
Wi-Fi or over a USB cable, and plays it there with as little delay as the two
operating systems allow.

## Status

**Both ends are built. Neither has been heard.**

What exists and is verified on a machine:

- **Windows capture.** WASAPI loopback with a silent render stream to keep the
  engine clocking. Three seconds of wall time produces exactly 144000 frames at
  48 kHz across 300 blocks, and a 440 Hz tone playing on the desktop is
  captured at -21.1 dBFS.
- **The desktop send path, end to end.** `cargo run -p sonduit-desktop --example
  bridge_loopback` opens real capture, sends over a real socket, decodes what
  arrives and writes a WAV. Three seconds: 501 datagrams sent, 501 received,
  none malformed, none lost.
- **The Android app.** Compose UI, a `mediaPlayback` foreground service, AAudio
  output, and a receive path tested against real UDP sockets. The debug APK
  assembles with the Rust library inside it.
- **Drift correction that works.** A simulated ten-minute session at 50 ppm,
  which empties an uncorrected buffer completely, settles within 3 ms of target
  in both directions.
- **The audio editor.** Convert, master, trim and modify, run through a bundled
  LGPL FFmpeg. Every mode was verified against the real binary.
- 196 Rust tests, full CI, release automation and language linting.

What has not been done:

- **No audio has ever been played through this.** There is no Android device
  here. Every latency figure below is a budget, not a measurement, and whether
  AAudio grants exclusive low-latency mode on any real phone is unknown. See
  [docs/environment.md](docs/environment.md).
- **USB tethering has never been tried.** Carrier entitlement can veto it, and
  that is the largest risk to [ADR-004](docs/adr/ADR-004-transport.md).
- **The audio is not encrypted.** Pairing stops an unpaired device being
  chosen, so nobody receives the stream by accident. It does nothing about
  anyone who can already see the traffic: the PCM is in the clear on the wire
  and reconstructing it is trivial. Fine on a home network or a USB cable;
  treat a shared or public network as if the audio were audible in the room.
- **Windows does not see the phone as a selectable output device.** That
  requires a signed kernel driver, and the prebuilt driver this project
  intended to reuse turns out to be unusable. See
  [ADR-002](docs/adr/ADR-002-desktop-capture.md).

## Targets

| Transport | Target, mouth to ear |
| --- | --- |
| Wi-Fi | 40-80 ms |
| USB tethering | 25-50 ms |

Bluetooth is explicitly out of scope as a transport.

## Layout

```text
crates/
  sonduit-core/               protocol, ring buffer, jitter, drift, resampling. No I/O, no platform code
  sonduit-transport/          UDP, discovery, sources and sinks
  sonduit-capture-win/        WASAPI capture
  sonduit-playback-android/   AAudio playback
  sonduit-ffi/                UniFFI surface for the Android app
desktop/                      Tauri v2 app: src/ React frontend, src-tauri/ Rust shell
android/                      Gradle project: Kotlin and Compose, Rust through UniFFI
driver/                       vendored driver and install scripts (empty, see ADR-002)
docs/                         architecture decisions, research, protocol, budget
tools/                        linting and version derivation
```

## Building

### What you need

For the shared core and the transport, which is most of the interesting code:

- Rust 1.85 or newer

That is genuinely all. `sonduit-core` has no platform dependencies and does no
I/O, so it builds and tests anywhere, including on a machine with no sound card
(see [ADR-001](docs/adr/ADR-001-shared-core-language.md)).

```bash
cargo test --workspace
```

For the desktop app, additionally:

- Node.js 22 or newer
- On Windows: Visual Studio Build Tools with the C++ workload, and the Windows
  SDK

```bash
cd desktop
npm ci
npm run tauri dev
```

For the Android app, additionally:

- JDK 17
- Android SDK, platform 34 or newer
- Android NDK r27 or newer, with `ANDROID_NDK_HOME` set (CI pins 27.2.12479018)
- `cargo install cargo-ndk`
- `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`

A physical device is required to judge anything about latency. An emulator is
enough to prove the app starts and no more.

### Checks

Run before every commit. CI runs the same set.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

node tools/lint/check-source-ascii.mjs
node tools/lint/check-commits.mjs HEAD~1..HEAD
node --test tools/version.test.mjs
node tools/version.mjs check
```

## Documentation

Start here:

- [docs/protocol.md](docs/protocol.md) - the wire format, byte for byte. The
  single source of truth.
- [docs/latency-budget.md](docs/latency-budget.md) - where every millisecond is
  meant to go. The measure every pull request is judged against.
- [docs/adr/](docs/adr/) - the architecture decisions and why they were made,
  including three places where research overturned the original plan.
- [docs/research/](docs/research/) - the evidence behind those decisions, with
  sources and an explicit list of what could not be verified.
- [docs/licensing.md](docs/licensing.md) - what is taken from where, and the
  decisions that would force this project to GPL.
- [docs/environment.md](docs/environment.md) - what can and cannot be built and
  verified, and therefore what to distrust.
- [docs/roadmap.md](docs/roadmap.md) - what is next, riskiest first.
- [CONTRIBUTING.md](CONTRIBUTING.md) - layering rules, commit format and
  language conventions.

## Licence

MIT. See [LICENSE](LICENSE).

Sonduit reads the Scream project's MS-PL driver source to document the wire
protocol it speaks. It contains no code from any GPL-licensed project, and the
reasoning is written down in [docs/licensing.md](docs/licensing.md).
