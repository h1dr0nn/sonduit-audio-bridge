# Sonduit

Low latency system audio bridge from Windows to Android.

Sonduit sends what your PC is playing to a phone on the same network, over
Wi-Fi or over a USB cable, and plays it there with as little delay as the two
operating systems allow.

## Status

**Early scaffolding. Not usable yet.**

What exists and is verified:

- The shared Rust core: wire protocol, ring buffer, adaptive jitter buffer and
  drift estimation, with 83 tests including an end-to-end run over a real UDP
  socket that asserts on the resulting WAV file.
- The desktop shell: it builds, runs, and renders. Custom window chrome,
  native acrylic backdrop, light and dark themes, eleven languages.
- Full CI, release automation and language linting.

What does not exist yet:

- Audio capture on Windows. `sonduit-capture-win` is scaffolding.
- The Android app. `android/` holds only `gradle.properties`; there is no
  Gradle project yet.
- Therefore: **no audio has ever been played through this.** See
  [docs/environment.md](docs/environment.md).

Two things you should know before forming expectations:

- **Windows does not yet see the phone as a selectable output device.** That
  requires a signed kernel driver, and the prebuilt driver this project
  intended to reuse turns out to be unusable. The reasoning is in
  [ADR-002](docs/adr/ADR-002-desktop-capture.md).
- **The latency figures below are budgets, not measurements.** Nothing has been
  measured. See [docs/latency-budget.md](docs/latency-budget.md).

## Targets

| Transport | Target, mouth to ear |
| --- | --- |
| Wi-Fi | 40-80 ms |
| USB tethering | 25-50 ms |

Bluetooth is explicitly out of scope as a transport.

## Layout

```text
crates/
  sonduit-core/               protocol, ring buffer, jitter, drift. No I/O, no platform code
  sonduit-transport/          UDP, discovery, sources and sinks
  sonduit-capture-win/        WASAPI capture
  sonduit-playback-android/   AAudio playback
  sonduit-ffi/                UniFFI surface for the Android app
desktop/                      Tauri v2 app: src/ frontend, src-tauri/ shell
android/                      Gradle project (not written yet)
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
