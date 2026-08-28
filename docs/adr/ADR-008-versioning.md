# ADR-008: Versioning, changelogs and release flow

- Status: accepted, with **one correction to the proposed formula**; the branch
  model and the develop version string were both **amended on 2026-08-28**
- Date: 2026-08-27
- Amended: 2026-08-28, see [Branches](#branches)
- Amended: 2026-08-28, see [Version strings](#version-strings)
- Supersedes: this ADR's own three-branch model, `main` / `develop` /
  short-lived branches into `develop`; and its own commit-implied develop
  version

## Context

Bumping the patch number on every develop build burns through the release
number space and makes `v1.2.7` ambiguous: nobody can tell whether it is a
release or a dev build.

## Decision

### Version strings

| Build | Format | Example |
| --- | --- | --- |
| Release, tagged on `main` | `release-va.b.c` | `release-v2.0.0` |
| Develop build | `va.b.c-dev.N+<sha>` | `1.3.0-dev.42+9f316eda` |

Develop builds are published under one rolling tag, `develop-build`, which each
`develop-vX.Y.Z` tag overwrites. It was originally every push to the `develop`
branch; the amendment under [Branches](#branches) is why that changed. The
prerelease tag is deliberately not called `develop`: a tag sharing a name with
a branch makes every unqualified reference to it ambiguous, and while that
branch existed git refused to push such a tag at all.

`N` is the number of commits since the last release; see below for why not the
CI run number. The `a.b.c` in front of it was amended.

**Amended 2026-08-28.** What was decided on 2026-08-27 is kept below rather
than overwritten, for the same reason as under [Branches](#branches): an ADR is
a record.

As originally decided:

> `1.3.0` is the **next** version implied by the Conventional Commits
> accumulated since the last release. This is valid SemVer, sorts correctly
> (`1.3.0-dev.42 < 1.3.0`), and a dev build can never be mistaken for a
> release.

As amended, and as it stands:

> `1.3.0` is **the version in the tree**, `[workspace.package] version` in the
> root `Cargo.toml`, unchanged. The commits in the range are counted for `N`
> and are not otherwise consulted. This is valid SemVer, sorts correctly
> (`1.3.0-dev.42 < 1.3.0`), and a dev build can never be mistaken for a
> release.

**Why it changed**, in the maintainer's terms: *if it is develop then make it
build 1.1.x; only raise it to 2.0 when releasing.*

The original formula was `applyBump(workspaceVersion, bumpFromCommits(range))`,
so a single commit carrying a `!` or a `BREAKING CHANGE:` footer anywhere in
the range forced the whole develop line to the next major. One such commit,
`81202e98`, sits inside `harmonix-final..HEAD`. Against a 1.1.0 tree that made
`version.mjs dev` print `2.0.0-dev.54`, so a `develop-v1.1.0` tag would have
published `sonduit_windows_2.0.0-dev.54_x86_64_portable.zip` -- artifacts named
for a release nobody had decided to cut.

Two things follow from the version being a decision rather than a computation:

1. **A develop build is a test build of the version the project is on**, not a
   preview of the next release. Naming it after a release that has not been
   decided asserts a decision the maintainer has not made.
2. **It puts the develop version back in agreement with the tag check.**
   `develop.yml` verifies the `develop-vX.Y.Z` tag against
   `node tools/version.mjs read`, which is the tree version. Deriving the
   artifact name from a different number meant the tag and the files it
   produced could name two different releases -- and did.

The commit-implied version is not lost: `node tools/version.mjs next` still
prints it, which is what a maintainer deciding the release version wants. It
simply no longer names anything on its own.

**What this amendment does not touch.** The versionCode layout, the dev
counter, and the changelog policy are unchanged. `N` is still commits since
the last release, and a develop build still sorts below the release of the
same version.

### The version lives in exactly one place

`[workspace.package] version` in the root `Cargo.toml`. Four things are
**derived** from it by `tools/version.mjs sync`:

| Derived | What is written |
| --- | --- |
| `desktop/src-tauri/tauri.conf.json` | the `version` field |
| `android/gradle.properties` | `sonduitVersionName`, and the versionCode below |
| `README.md` | the shields.io version badge |
| the root `Cargo.toml` itself | the `version` on each `sonduit-*` workspace dependency, which must match the crates in this tree |

`tools/version.mjs check` re-reads each of them and fails CI on any
disagreement. For `gradle.properties` it checks `sonduitVersionName`; the
versionCode beside it is a pure function of that name, so checking both would
check the same thing twice. The README badge is derived for the same reason as
everything else here: a version number typed into a readme by hand is one that
goes stale silently and says nothing when it does.

### Android versionCode: the proposed formula was not monotonic

The formula in the brief was:

```text
code = major * 1_000_000 + minor * 10_000 + patch * 100 + devN
```

**This is broken, and a test caught it.** Counterexample:

```text
1.3.0-dev.7  ->  1030007
1.3.0        ->  1030000     <-- lower than its own dev builds
```

A build named `1.3.0-dev.N` is a preview of the *upcoming* 1.3.0, so it is
published first and must sort below it. Play Store rejects a versionCode it has
already seen, and the mistake is unrecoverable because a code can never be
reused. Under the original formula, the first real release after any dev build
of that version would be rejected permanently.

**The corrected layout puts the release at the top of each version block:**

```text
code = major * 10_000_000 + minor * 100_000 + patch * 1_000 + dev

  dev = 0..998   develop builds
  dev = 999      the release
```

Every field must fit strictly inside the one above it:

| Field | Range | Must stay below | Because |
| --- | --- | --- | --- |
| `dev` | 0..999 | 1_000 | the patch multiplier |
| `patch` | 0..**99** | 100 | so `patch*1_000` stays under 100_000 |
| `minor` | 0..99 | 100 | so `minor*100_000` stays under 10_000_000 |

**A second overflow was found in review, one field down.** The first correction
allowed `patch` up to 999, which breaks the same way:

```text
1.0.999  ->  10_999_999
1.1.0    ->  10_100_999     <-- lower, though it ships later
```

and it collided outright, `0.9.99` and `0.0.999` both mapping to `999_999`.
The tests passed because they only walked the six examples printed in this
document. They now walk the whole representable space and assert no collisions.

Verified monotonic across dev builds, their release, the next patch and a major
bump:

```text
1.2.3         -> 10203999
1.3.0-dev.1   -> 10300001
1.3.0-dev.998 -> 10300998
1.3.0         -> 10300999
1.3.1-dev.1   -> 10301001
2.0.0         -> 20000999
```

**Ceiling:** Play Store caps versionCode at 2,100,000,000. The largest code a
given major can produce is `major*10_000_000 + 9_999_999`, so **major may go up
to 209** (209 gives 2,099,999,999).

### Lowering the version forces a reinstall on every phone

The versionCode is a pure function of the version, so **lowering the version
lowers the versionCode**, and Android's package manager refuses an APK whose
versionCode is below the one already installed. It fails the install outright
with `INSTALL_FAILED_VERSION_DOWNGRADE`; there is no override on a release
build. Play Store is stricter still: a code below one it has already accepted
cannot be uploaded at all, and no code is ever reusable.

Worked example, the one this project actually performed. Setting the workspace
version back from 2.0.0 to 1.1.0 moves the codes down by nine million:

```text
2.0.0        -> 20000999      1.1.0        -> 10100999
2.0.0-dev.38 -> 20000038      1.1.0-dev.38 -> 10100038
```

**The consequence is not avoidable and must not be worked around.** Any scheme
that keeps the number climbing while the version goes down -- an offset, a
build counter smuggled into a field, a floor -- breaks the property the layout
exists to guarantee, which is that the code and the version say the same thing.
The version is the decision; the code follows from it.

So a downward version change costs one manual uninstall of the previous build
on every device carrying it, and the app data goes with it. That is cheap while
nothing has been published, and it is the reason a version is lowered then or
not at all.

### The dev counter is commits since the last release, not the CI run number

The obvious choice, `github.run_number`, is wrong twice over. It grows without
bound across the life of the repository, so it eventually reaches **999 and
collides with the reserved release slot** — producing a code Play Store has
already accepted — and then **1000, which falls outside the field entirely** and
makes every develop build fail.

Commits since the last release is monotonic within a release cycle and resets
at every release, which is exactly the property the field needs. Exceeding 998
commits in one cycle throws with a clear message rather than wrapping.

The cost: **a develop build cannot be reproduced locally with the same version
string** unless the same commits are present. Accepted, because the embedded
short SHA identifies the build precisely and is reproducible.

### Changelogs are for minor and major releases only

Per the maintainer's decision:

| Build | Changelog |
| --- | --- |
| Develop build | **none** |
| Patch release | **none**; notes point at the commit history |
| Minor release | generated by git-cliff |
| Major release | generated by git-cliff |

The reasoning is that a develop build has nothing worth summarising,
and a patch release is fixes and chores whose commit list is already the
clearest description available. Generating a changelog for both produces noise
that trains people to ignore changelogs.

`release.yml` **fails** when git-cliff produces an empty changelog for a minor
or major release. A release with a useless changelog is worse than a failed
build.

### The Harmonix boundary, and a bug it caused

This repository previously held Harmonix SE, tagged `v1.0.0` through `v1.0.4`,
with commit messages that are not Conventional Commits.

Two mechanisms keep them out:

1. **git-cliff always receives an explicit range**, falling back to
   `harmonix-final..<tag>` when there is no previous Sonduit release. Verified:
   an unbounded run reaches Harmonix content, a bounded one does not.

2. **`lastReleaseTag()` excludes tags reachable from `harmonix-final`.** This
   is not cosmetic. `git describe --match 'v[0-9]*.[0-9]*.[0-9]*'` matches
   `v1.0.4`, which is numerically higher than any early Sonduit version, so the
   next release would have computed as **1.1.0 instead of 0.2.0**. Identifying
   Harmonix tags by reachability rather than by guessing at numbers is what
   makes this correct.

   Since the release tag format became `release-vX.Y.Z`, `lastReleaseTag()`
   lists `release-v[0-9]*.[0-9]*.[0-9]*` and a Harmonix `v1.0.x` tag no longer
   matches the pattern in the first place. The reachability check stays as the
   second line of defence: the tag format is a convention and conventions get
   changed, whereas `harmonix-final` is a fact about the history.

The Harmonix git tags are **kept**. History is the only archive, and
`harmonix-final` is load-bearing for both mechanisms above. Only the GitHub
*Release* objects were removed.

### Branches

**Amended 2026-08-28.** What was decided on 2026-08-27 is kept below rather
than overwritten: an ADR is a record, and a model that was tried and reversed
is worth more than one that looks as though it had always been this way.

As originally decided:

| Branch | Role |
| --- | --- |
| `main` | protected, releases only, every commit tagged |
| `develop` | integration; everything lands here first, published as `develop-build` |
| `feat/*`, `fix/*`, `chore/*` | short-lived, PR into `develop` |

As amended, and as it stands:

| Branch | Role |
| --- | --- |
| `main` | the only branch. Everything lands here |
| `feat/*`, `fix/*`, `chore/*` | short-lived, PR into `main` |

`develop` is deleted. There is no integration branch, and there is no release
branch.

**Why it changed**, in the maintainer's terms:

1. **A push to `main` should not start a build.** `develop` existed largely to
   be the branch that pushes landed on, and every push to it cut a build.
   Collapsing to one branch while keeping that trigger would have meant every
   commit on `main` building something, which is not wanted. So the trigger
   moved off pushes entirely rather than moving onto `main`.
2. **A release should be one deliberate act.** A tag is already that act. The
   branch, the pull request and the integration branch around it were ceremony
   that added nothing to it.

Nothing is built by pushing, on any branch. Builds are cut by a tag and by
nothing else:

| Tag | What it does |
| --- | --- |
| `develop-vX.Y.Z` | builds a test artifact and overwrites the rolling `develop-build` prerelease |
| `release-vX.Y.Z` | builds, generates the changelog for a minor or major, publishes the release |

Checked against `.github/workflows/`: `develop.yml` triggers on
`push: tags: ['develop-v[0-9]+.[0-9]+.[0-9]+']`, `release.yml` on the same
shape of `release-v` tag, and `ci.yml` on `pull_request` only. No workflow
triggers on a push to a branch.

**What this amendment does not touch.** The three things this ADR is actually
about -- the version-string formats, the Android versionCode layout, and the
changelog policy -- are unchanged. Only the route a commit takes to a build is
different, and both tags are still checked against the version in the tree.

### Releasing

A tag, and nothing else. Pushing `release-vX.Y.Z` on `main` triggers
`release.yml`, which builds, generates the changelog for a minor or major, and
publishes. The version in `Cargo.toml` is bumped in an ordinary commit before
the tag; `release.yml` refuses the tag if the two disagree, so the two cannot
drift apart silently.

There was briefly a `release-pr.yml` that computed the bump, drafted the
changelog and kept a `Release vX.Y.Z` pull request open from a `release/vX.Y.Z`
branch. It was removed. Automating the bump bought little -- it is one line in
one file -- and it paid for that with a branch and a pull request nobody asked
for appearing in the repository on every push to the integration branch. That
integration branch is gone too now, for the reason above; the workflow, the
`release/*` branches and the release pull requests went with it.

## Consequences

- The release version is edited by hand, once, in `Cargo.toml`. Everything
  else is derived from it, and CI fails if the derived files disagree or if the
  tag disagrees with the file.
- 998 develop builds per release cycle is a real ceiling, and
  `devCounter` in `tools/version.mjs` throws with a clear message rather than
  wrapping into the release slot. At one build per `develop-vX.Y.Z` tag -- a
  deliberate act, not a push -- reaching it is implausible, and cutting a
  release resets it.
- **A version that goes down cannot be installed over one that went up.** The
  versionCode follows the version, Android refuses a downgrade, and the fix is
  a manual uninstall on every device holding the higher build -- not a change
  to the formula. See
  [Lowering the version forces a reinstall on every phone](#lowering-the-version-forces-a-reinstall-on-every-phone).
- Patch releases have thin release notes by design. If that proves unhelpful,
  the change is one branch in `release.yml`.
