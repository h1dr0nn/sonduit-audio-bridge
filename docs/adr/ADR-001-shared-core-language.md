# ADR-001: Rust for the shared core

- Status: accepted
- Date: 2026-08-27
- Deciders: project maintainer

## Context

Sonduit runs on two platforms that share nothing: a Windows desktop sender and
an Android receiver. Between them sits logic that is identical on both sides
and unforgiving of error: packet encode and decode, a ring buffer, an adaptive
jitter buffer, drift estimation and clock comparison.

If that logic exists twice, the two copies will disagree. A wire format
mismatch is a silent corruption, and a jitter buffer that behaves differently
on each end is close to undebuggable once a real network sits between them.

## Decision

**One shared core, written in Rust, compiled for both Windows and Android.**

`sonduit-core` owns the latency-critical logic and is depended on by every
other crate.

**`sonduit-core` has no platform dependencies and performs no I/O.** No
sockets, no audio APIs, no threads, no clocks. Time and packet arrivals are
passed in by the caller.

## Consequences

### Why the no-I/O rule matters more than the language choice

The development machine has no JDK, no Android SDK or NDK, no `cargo-ndk`, and
no Android device (see `environment.md`). If the jitter buffer needed a socket
or a sound card to run, none of it could be exercised here at all.

Because it takes arrival times as arguments, the whole of it is testable
against a synthetic packet timeline: reordering, loss, duplicates, sequence
wrap, and a steadily skewing clock. Two real design defects were found that way
during the first implementation, both of which produced plausible wrong answers
rather than crashes.

### Enforcement

Discipline is not a control. CI reads `cargo tree -p sonduit-core` and fails if
`windows`, `windows-sys`, `ndk`, `oboe`, `jni`, `tokio`, `winapi` or
`core-foundation` appear. `deny.toml` restates the rule for `tokio`.

Any pull request adding a platform dependency to the core should be rejected.

### The licence dimension

`sonduit-core` compiles into **both** shipped binaries, so a copyleft
dependency anywhere in its tree contaminates the desktop product as well as the
mobile one. Its dependency list is the highest-value thing in the repository to
keep clean, which is a second reason for the "few carefully chosen crates"
rule. See `licensing.md`.

### Costs accepted

- Rust on Android needs `cargo-ndk` and a UniFFI boundary, which is more build
  machinery than a pure Kotlin receiver would need.
- Contributors must know Rust to touch the audio path.
- The core currently depends on `thiserror` only.

## Alternatives rejected

- **C++ shared core.** Portable and fast, but no memory-safety guarantee on
  code that runs in a realtime callback, and a far worse cross-compilation and
  dependency story.
- **Kotlin Multiplatform.** Would put a garbage collector in the audio path.
- **Reimplement per platform.** The failure mode this ADR exists to prevent.
