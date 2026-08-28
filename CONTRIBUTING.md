# Contributing to Sonduit

## Start here

Once, when you clone:

```bash
git config core.hooksPath tools/hooks
```

That makes `tools/hooks/commit-msg` reject a message the project's own linter
would reject. It is not optional in practice: history here is the only archive
there is, so a bad subject line cannot be fixed after the fact, and four of them
reached `main` before the hook existed. They are listed in
`tools/lint/commit-baseline`.

Before every commit:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node tools/lint/check-source-ascii.mjs
node tools/version.mjs check
```

On Linux add `--exclude sonduit-desktop` to the clippy and test commands: it is
a Tauri application and wants GTK and webkit headers there for no benefit.

**Never commit while red. Never commit code that has not been run.**

## The layering rule

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

**Dependencies point downward only.** A lower layer never reaches up.

### `sonduit-core` is the one that matters

**It has no platform dependencies and does no I/O.** No sockets, no audio APIs,
no threads, no clocks. Time and packet arrivals are passed in by the caller.

This is not tidiness. It is what makes the jitter and drift logic testable
against a synthetic packet timeline on a machine with no audio hardware, which
is the only kind of machine this project's CI has. Two real design defects were
found that way, both producing plausible wrong answers rather than crashes.

CI reads `cargo tree -p sonduit-core` and **fails** if `windows`, `windows-sys`,
`ndk`, `oboe`, `jni`, `tokio`, `winapi` or `core-foundation` appear in it.

A pull request that adds a platform dependency to the core will be rejected.
See [ADR-001](docs/adr/ADR-001-shared-core-language.md).

### Dependencies need justification

Prefer few, carefully chosen crates over many. A new dependency belongs in the
relevant ADR with a reason.

`sonduit-core` compiles into **both** shipped binaries, so a copyleft
dependency anywhere in its tree contaminates the desktop product as well as the
mobile one. `cargo deny` enforces a permissive-only licence allowlist. See
[docs/licensing.md](docs/licensing.md).

## The realtime contract

Code that runs in an audio callback must not:

- allocate
- take a lock
- perform any I/O
- call JNI
- sleep or block
- panic

`sonduit-core` carries `forbid(unsafe_code)`. The ring buffer exists so the
network thread and the audio callback can hand data across without any of the
above.

## Language

**English** for: identifiers, type and function names, file and directory
names, branch names, commit messages, pull request titles and bodies, ADRs,
everything in `docs/`, the README, the changelog, log and error strings, and CI
job names.

**Vietnamese is allowed** in plain `//` inline comments, for explaining hard
domain logic: jitter maths, drift correction, protocol quirks.

**Vietnamese is not allowed** in `///` rustdoc or KDoc documentation comments,
because those become public API documentation.

This is enforced, not trusted:

```bash
node tools/lint/check-source-ascii.mjs
```

It fails on non-ASCII anywhere in a `.rs` or `.kt` file except inside a plain
`//` comment body, and it understands the difference between a comment and a
string literal.

## Commits

Conventional Commits 1.0.0, enforced in CI on every commit in a pull request.

```text
<type>(<scope>): <subject>
```

| Field | Values |
| --- | --- |
| type | `feat` `fix` `perf` `refactor` `docs` `test` `build` `ci` `chore` `revert` |
| scope | `core` `transport` `capture-win` `playback-android` `desktop` `android` `driver` `protocol` `ci` `docs` `deps` |
| subject | imperative, lowercase, no trailing period, whole line at most 72 characters |

Breaking changes are `feat(core)!: ...` or a `BREAKING CHANGE:` footer.

**Every commit must build and pass tests on its own.** No `wip` or `fix typo`
commits; squash before you push. The linter rejects both.

```bash
node tools/lint/check-commits.mjs origin/main..HEAD
```

## Placeholders

Do not write a function that quietly returns `Ok(())`. If it is not implemented
it is `todo!()`, and it is listed in
[docs/roadmap.md](docs/roadmap.md) section 4.

A silent no-op is worse than a panic, because it looks like it works.

## Latency

[docs/latency-budget.md](docs/latency-budget.md) allocates every millisecond
from capture to speaker.

**A pull request that adds latency must say which stage and how much, and
update that table.** Pushing the total past the target band requires a
compensating reduction elsewhere or an explicit decision to move the target.

## Branch, tags and versions

One branch: `main`. Short-lived `feat/*`, `fix/*` and `chore/*` branches open a
pull request against it. There is no integration branch, and there is no
release branch: a workflow used to open a release pull request from one, and
the branch it left behind was noise nobody had asked for.

Nothing is built by pushing. Builds are cut by a tag and by nothing else:

| Tag | What it does |
| --- | --- |
| `develop-vX.Y.Z` | builds a test artifact and overwrites the rolling `develop-build` prerelease |
| `release-vX.Y.Z` | builds, generates the changelog for a minor or major, publishes the release |

Both tags are checked against the version in the tree, so a tag cannot name a
release that does not exist.

The version lives in exactly one place, `[workspace.package] version` in the
root `Cargo.toml`. `tauri.conf.json`, `android/gradle.properties` and the
README badge are derived by `tools/version.mjs sync`, and CI fails if they
drift.

**Never edit a version by hand in two files.** See
[ADR-008](docs/adr/ADR-008-versioning.md).

## Never

- `git push --force`, `git reset --hard`, `git filter-branch` or `git rebase`
  on `main` or `archive/*`. **In this repository the history is the backup**;
  rewriting it destroys the only copy of the Harmonix SE project this
  repository used to hold.
- Copy code from a GPL-licensed project. `third_party/reference/scream-android`
  is GPL-3.0 and `third_party/reference/AndroidUsbAudioDevice` has no licence
  at all, which is more restrictive still. Both are gitignored and are read
  only to establish facts about protocols, never for implementation.
- Modify the vendored driver. MS-PL permits it; Windows signing makes it
  impractical. See [ADR-002](docs/adr/ADR-002-desktop-capture.md).
