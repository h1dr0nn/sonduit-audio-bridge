# ADR-007: Repository layout and build system

- Status: accepted
- Date: 2026-08-27

## Decision

```text
sonduit-audio-bridge/
  Cargo.toml                    workspace root; the version lives here
  crates/
    sonduit-core/               protocol, ring, jitter, drift. No I/O, no platform
    sonduit-transport/          UDP, discovery, sources and sinks
    sonduit-capture-win/        WASAPI capture
    sonduit-playback-android/   AAudio playback
    sonduit-ffi/                UniFFI surface for the Android app
  desktop/
    src/                        React frontend
    src-tauri/                  Tauri shell, a workspace member
  android/                      Gradle project, Compose UI, cargo-ndk
  driver/                       vendored driver and install scripts
  docs/                         adr/, research/, protocol, licensing, budget
  tools/                        lint/, version.mjs and its tests
  .github/workflows/
```

### Layering rule

```text
desktop, android
      |
      v
sonduit-ffi, sonduit-capture-win, sonduit-playback-android
      |
      v
sonduit-transport
      |
      v
sonduit-core
```

Dependencies point downward only. `sonduit-core` depends on nothing in this
list, and CI enforces that it depends on no platform crate either (ADR-001).

### Why `desktop/src` and `desktop/src-tauri`

The repository previously had `frontend/` and `src-tauri/` at the root, which
is not the Tauri convention and left the Tauri app unable to find its own
frontend. Nesting both under `desktop/` restores the conventional pairing and
makes `beforeBuildCommand: npm run build` correct without path gymnastics.

### One Cargo workspace, including the Tauri crate

`desktop/src-tauri` is a workspace member, so `cargo test --workspace` covers
everything and there is a single `Cargo.lock`. `cargo-ndk` drives the Android
cross-compile of `sonduit-ffi`; Gradle wraps that.

### Profiles

`sonduit-core` and `sonduit-transport` are compiled at `opt-level = 2` **even
in dev**. A debug-speed jitter buffer cannot keep up with realtime audio, so a
debug build would be useless for judging behaviour and its tests unbearably
slow.

### Platform crates compile everywhere

`sonduit-capture-win` and `sonduit-playback-android` gate their platform code
behind `cfg` and compile to their platform-independent parts elsewhere. That
keeps `cargo build --workspace` working on the Linux CI runner and on any
contributor's machine, rather than requiring a matrix just to type-check.

## Consequences

- A single lockfile means the Android and Windows halves share dependency
  versions, which is what we want given they share a core.
- The Tauri crate being a workspace member means `cargo clippy --workspace`
  lints it too.
- `third_party/reference/` is gitignored. It holds a GPL-3.0 repository and one
  with no licence at all, and neither may ever be committed (`licensing.md`).
- `driver/` exists but is empty pending ADR-002.
