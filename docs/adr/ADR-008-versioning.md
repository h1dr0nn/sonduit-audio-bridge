# ADR-008: Versioning, changelogs and release flow

- Status: accepted, with **one correction to the proposed formula**; the branch
  model, the develop version string, **when the tree version moves** and
  **what the third component counts** were all **amended on 2026-08-28**
- Date: 2026-08-27
- Amended: 2026-08-28, see [Branches](#branches)
- Amended: 2026-08-28, see [Version strings](#version-strings)
- Amended: 2026-08-28, see
  [When the tree version moves](#when-the-tree-version-moves)
- Amended: 2026-08-28, see [The counter lives in c](#the-counter-lives-in-c)
- Supersedes: this ADR's own three-branch model, `main` / `develop` /
  short-lived branches into `develop`; its own commit-implied develop
  version; its own rule that the version in the tree moves only when a
  release is cut; and its own `a.b.c-dev.N` develop version string, whose
  counter now lives in `c`

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

**Superseded 2026-08-28 by [The counter lives in c](#the-counter-lives-in-c).**
The develop row of that table is the record of what was decided, not what is
built: there is no `-dev.N` suffix any more, and a develop build carries the
plain `a.b.c` of the release row, with the counter in `c`. What still stands
here is the rolling `develop-build` tag, the reason the prerelease tag was
never called `develop`, and the rule the second amendment established -- that
the base is the version in the tree and not what the commits imply.

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

That `unchanged` means the develop build takes the tree version **verbatim**
rather than applying a bump to it. It is not a claim that the tree version
itself sits still between releases; see
[When the tree version moves](#when-the-tree-version-moves).

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
prints it, which is what a maintainer deciding the release version wants --
and, since [When the tree version moves](#when-the-tree-version-moves), what a
maintainer deciding a mid-development bump reads too, with the caveats recorded
there. It simply no longer names anything on its own.

**What this amendment does not touch.** The versionCode layout, the dev
counter, and the changelog policy are unchanged. `N` is still commits since
the last release, and a develop build still sorts below the release of the
same version.

### When the tree version moves

**Amended 2026-08-28.** The third amendment of the day, in the same shape as
the two above: the original rule is kept legible rather than overwritten,
because it was reasoned, and because what went wrong with it is only visible
next to it.

As originally decided:

> The number in the tree moves **when a release is cut, and at no other time**.
> Development does not touch it; a develop build distinguishes itself with the
> `-dev.N` counter instead, so the release number space is not burned through
> by ordinary work.

As amended, and as it stands:

> The version in the tree **moves with accumulated work**, by the same
> Conventional Commits rule the release tooling already applies: a batch of
> `feat` is a minor bump, a batch of `fix` a patch bump. It stays a deliberate
> edit in an ordinary commit -- one line in one file, then
> `node tools/version.mjs sync`. `2.0.0` remains reserved for the first real
> release, which is the maintainer's call and **is not reached by
> accumulation**.

**Why the original rule was reasonable.** It keeps the release number space
clean, it keeps `v1.2.7` unambiguous -- which is the whole of the
[Context](#context) above -- and it makes the version a decision rather than a
computation, which is what the [Version strings](#version-strings) amendment
exists to protect. For a project that releases on any regular cadence it is the
right rule, and none of that reasoning has been shown wrong.

**What made it wrong here** is one fact the rule did not anticipate: **nothing
has ever been released.** `git tag --list 'release-v*'` is empty. Under "move
it at a release" the number therefore never moved at all: the tree sat at
1.1.0 through five `feat` and nine `fix` commits, and the number on screen and
in the About panel said nothing whatever about what the build contained. In
the maintainer's terms: *I have edited so many things and the version still
hasn't gone up.* **A rule whose correctness depends on releasing regularly is
the wrong rule for a project that has not released once.**

The change is narrow, and it is worth being precise about how narrow. It is
not a finding that versions should track commits in general. It is that a
number nobody ever moves is not a version, it is a constant, and a constant on
the About panel is worse than no number at all.

**Why the bump was 1.2.0 and not 2.0.0.** `node tools/version.mjs next` prints
`2.0.0`. It is correct by its own rule and wrong for this decision. Two
commits in `harmonix-final..HEAD` carry a breaking-change marker:

| Commit | Marker | What it breaks |
| --- | --- | --- |
| `81202e9` `chore!` | `BREAKING CHANGE:` footer | nothing. It records that Harmonix SE predates all of this and that Sonduit is not a compatible successor |
| `ca16f29` `fix(core)!` | `!` on the subject | nothing. It removed an enum variant that could no longer be produced |

No crate is published and no build has shipped, so neither commit breaks
anything for anybody. Neither should drag the tree to a major, and `2.0.0` is
spoken for.

**The tooling does not compute this bump for you, and it is not going to be
made to.** `version.mjs next` answers exactly one question -- what do the
commits since the last *release* imply, applied to the tree version -- and now
that the tree version also moves between releases, that range spans commits an
earlier mid-development bump has already accounted for. So `next` re-counts
them and compounds: setting the breaking-change markers above aside, the five
`feat` commits that justified 1.1.0 -> 1.2.0 would imply 1.3.0 from the
resulting 1.2.0 tree without a single new commit. No command computes a
"since the last bump" range, and nothing records where the last bump was.
`next` is therefore a prompt, not an answer: read the range and decide. That is
the same position the [Version strings](#version-strings) amendment took, for
the same reason -- the version is a decision, and a tool that guessed it would
be asserting one nobody made.

**Nothing else in the tooling objects.** Verified against
`tools/version.mjs` and `.github/workflows/`: `version.mjs check` passes on a
mid-development bump, `sync` rewrites the four derived files as usual, and both
tag checks -- `develop.yml` and `release.yml`, each comparing the tag against
`version.mjs read` -- are checks against the tree version, which is precisely
the number this amendment moves. A bump mid-development is an ordinary `sync`
and an ordinary commit.

**What this amendment changes elsewhere.** The version-string formats, the
versionCode layout and the changelog policy are unchanged. Two things further
down did change and are corrected in place with a pointer back here: what the
[dev counter](#the-dev-counter-is-commits-since-the-last-release-not-the-ci-run-number)
resets on, and what a version that moves upward mid-development does to the
[versionCode](#lowering-the-version-forces-a-reinstall-on-every-phone).

### The counter lives in c

**Amended 2026-08-28.** The fourth amendment of the day, in the same shape as
the three above: the original is kept legible, because what went wrong with it
is only visible next to it.

As originally decided:

> The version is `a.b.c` -- major, minor, **patch** -- and a develop build is
> told apart by a prerelease counter appended to it, `1.2.0-dev.74+9f316eda`,
> where `N` is commits since the last release. `N` is packed into the low field
> of the Android versionCode, which reserves 999 for "this is the release".

As amended, and as it stands:

> The version is `a.b.c` where **a is major, b is minor, and c is the develop
> build counter**. `c` advances by one per commit and **rolls into `b` at 100**.
> There is no prerelease suffix and no build metadata: a develop build and a
> release cut from the same commit carry the same string, `1.2.76`.

**Why it changed**, in the maintainer's terms: *a.b.c, a la major, b la
minor/develop, c la develop.* The number they want to read is `1.2.74`, not
`1.2.0-dev.74`.

The third amendment above had already moved the tree version during
development, so that the number on screen said something about what was in the
build. It left the *rate* wrong: the number moved when somebody decided to move
it, and the thing that actually advanced on every commit was hidden in a
suffix. `c` is where that count belongs. The suffix is deleted rather than
demoted, because two counters -- one in `c` and one in `-dev.N` -- would be two
numbers to keep in agreement, and one of them redundant.

**This gives something up, and it is worth naming.** The [Context](#context) at
the top of this ADR says a develop build must not be mistakable for a release,
and the `-dev.N` suffix was how the version string said so. It no longer does.
The distinction moved to the tag, the release object, the artifact name and the
versionCode slot; see
[Telling a release apart from a develop build](#telling-a-release-apart-from-a-develop-build)
below. What is lost is that a bare version string, quoted with no context, no
longer says which kind of build it came from. That is the maintainer's call, and
it buys the thing they asked for: one number, visible, that moves.

#### What c counts from

**The commit that last wrote the version into the tree.** Not the last release.

```text
c = the c written at the last bump + commits since that bump commit
```

The version records its own anchor. `git log -- Cargo.toml`, newest first, and
the anchor is the oldest commit of the newest unbroken run carrying the version
now in the tree. A bump still sitting in the working tree anchors to HEAD, which
is what makes `bump` idempotent: running it twice writes the same number.

Anchoring to the last *release* tag, which is what the old `-dev.N` counter did,
**cannot work once `c` rolls**, and the reason is the one the third amendment
recorded: there has never been a release tag, so that counter has never reset,
and a tree bump does not reset it. Walk it through. At 100 commits the counter
rolls the version to 1.3.0 and `bump` writes that into the tree. The next commit
asks the same question and gets the same answer -- 101 commits since
`harmonix-final` -- so it rolls 1.3.0 to 1.4.0. And again on the next. The
version would climb a minor per commit until `b` reached 99 and the tooling
threw.

So this amendment **solves the anchor problem that amendment named**. Where it
says *no command computes a "since the last bump" range, and nothing records
where the last bump was*, both halves are now false: `version.mjs anchor` prints
that range, and the version in the tree is the record. Corrected in place under
[The dev counter](#the-dev-counter-is-commits-since-the-last-release-not-the-ci-run-number).

The release range has not gone away. `version.mjs range` still computes it, and
the changelog and the commit-message lint still use it. It is simply not what
the build counter is measured over any more.

#### The rollover

`c` cannot pass 99, because `patch * 1_000` has to stay inside the 100_000
minor multiplier of the versionCode. So it carries:

```text
1.2.99 + 1 commit   ->  1.3.0        10_299_999  ->  10_300_999
1.2.99 + 2 commits  ->  1.3.1
1.2.0  + 250        ->  1.4.50
```

It is a **carry, not a reset**: the counter keeps advancing by one per commit
across the boundary whether or not anybody bumped the tree in between, and the
versionCode rises across it, which is the property that is not negotiable.

Two alternatives were considered and rejected:

1. **Throw at 99 and demand a manual minor bump.** This is what the old
   `devCounter` did at 998. It stops every build for a limit ordinary work
   reaches -- roughly every hundred commits -- and the fix it demands is exactly
   the carry, performed by hand.
2. **Widen the field so `c` can hold more.** Any widening divides the
   multipliers above it, which lowers every code the layout produces.
   `10_200_999` is installed on a phone right now. A scheme producing anything
   below it would not install, and a code that has been accepted is never
   reusable. The field widths are load-bearing and were not touched.

Rolling past `b = 99` throws, and that is correct: raising the major is a
decision, and the tooling does not make it.

#### What became of the dev field and the release slot

The layout is **unchanged**, for the reason in (2) above. What changed is what
the low field means. It was a counter with a reserved top value; it is now two
named slots with nothing in between:

| Slot | Value | Meaning |
| --- | --- | --- |
| `DEVELOP_SLOT` | 0 | a develop build of this a.b.c |
| unused | 1..998 | reserved, spent by nothing |
| `RELEASE_SLOT` | 999 | the release of this a.b.c |

`RELEASE_SLOT` survives and still earns its place. The version string no longer
distinguishes a develop build from a release, so if the code did not either,
`develop-v1.2.76` and `release-v1.2.76` -- a plausible pair, test it and then
ship it with no commit in between -- would produce two APKs with one code. The
release keeps the top of its block, so it outranks the develop build it follows
and installs over it. That is the reason the slot was invented, applied to the
one case that survives.

The 998 values in the middle are left unspent rather than reclaimed. Reclaiming
them means narrowing the field, which is alternative (2), which is refused.
Codes are spent and never reused, and the ceiling is 209 majors away.

`devCounter()` and `MAX_DEV` are gone with the suffix that used them. The
ceiling they guarded is now `c <= 99`, enforced by the carry rather than by an
exception.

#### Telling a release apart from a develop build

Four things say it. The version string is not one of them:

| Where | Develop | Release |
| --- | --- | --- |
| Tag | `develop-vX.Y.Z` | `release-vX.Y.Z` |
| GitHub | the rolling `develop-build` prerelease | a release object of its own |
| Artifact name | `sonduit_android_1.2.76-dev_universal.apk` | `sonduit_android_1.2.76_universal.apk` |
| versionCode | `10_276_000` | `10_276_999` |

The tag is the source of all four: it is what triggers the workflow, and the
workflow is what passes `--develop` to `version.mjs code` and `version.mjs
sync`. Nothing sniffs the version string for a suffix, because there is none to
sniff, and a check that did would be comparing a string with itself.

`develop.yml` had exactly such a check -- it verified that the computed develop
version had the shape `$declared-dev.*` -- and under this scheme it would have
passed unconditionally. It is replaced by `node tools/version.mjs check`, which
verifies that every derived file agrees with the tag, not only the one file the
tag was compared against.

#### What the tooling looks like now

| Command | Was | Is |
| --- | --- | --- |
| `read` | the tree version | unchanged |
| `anchor` | -- | the commit `c` counts from, and the count |
| `advance` | -- | the version HEAD implies; read-only |
| `bump [minor, major, version]` | -- | writes that version and syncs |
| `next` | the commit-implied version, from the last **release** | the commit-implied version, from the last **bump** |
| `dev` | `X.Y.Z-dev.N+sha` | **removed**; a develop build is `read` |
| `last-release` | the last release tag | **removed**; nothing called it, and `range` prints what it was for |
| `range`, `code`, `sync`, `check` | | unchanged, except that `code` and `sync` take `--develop` |

**`next` is an answer again.** The third amendment recorded it as a prompt,
because its range was "since the last release" and it therefore re-counted
commits an earlier mid-development bump had already accounted for. Its range is
now "since the last bump", a range that exists for the first time, so the
double-counting is gone. Its `patch` case returns the version unchanged -- `c`
has already moved, and there is no third number left to bump -- so what it
answers is the only question left: is this a minor, is it a major. The two
harmless `!` markers that amendment names sit behind the current anchor and no
longer reach it.

**A release with features but no `b` bump gets no changelog.** `release.yml`
classifies a release by comparing `a.b` against the previous release tag, so
1.2.76 following 1.2.40 is a patch release and its notes point at the commit
history. That is unchanged behaviour, and it stays correct under this scheme for
the same reason it was correct before: `b` is the minor **decision**, and a
maintainer who has accumulated a minor's worth of features bumps it. A release
that did not is one where that decision was not made.

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

**Corrected 2026-08-28.** The arithmetic and every field width below are
unchanged and are the reason nothing here could be widened. The low field is no
longer a counter: `dev = 0` means a develop build and `dev = 999` the release,
1..998 are spent by nothing, and the count that used to live there is `c`. See
[What became of the dev field and the release slot](#what-became-of-the-dev-field-and-the-release-slot).

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

The `-dev.N` names above cannot be produced any more, and the sequence they
walk is now the one in [The rollover](#the-rollover): 1.2.99 develop, 1.2.99
release, 1.3.0 develop, 1.3.0 release, which is `10299000`, `10299999`,
`10300000`, `10300999`. The tests walk both, in both slots, either side of the
carry.

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

**Upward is the direction the version now moves, and it is the safe one.**
Since [When the tree version moves](#when-the-tree-version-moves) the tree
version rises during development, and every rise raises the versionCode with
it. The phone was carrying `10100999` from the 1.1.0 tree; a synced 1.2.0 tree
stamps `10200999`, which installs over it with nothing to uninstall.
Monotonicity survives a mid-development bump for exactly the reason it survives
a release: the smallest bump, a patch, adds 1_000, and `N` can only ever add
998, so a later version cannot produce a lower code than an earlier one however
far the counter has run. Checked:

```text
1.1.0-dev.38  -> 10100038
1.2.0-dev.75  -> 10200075
1.2.1-dev.998 -> 10201998
1.2.2-dev.0   -> 10202000
```

That `10200999` is the release slot of a version this project does not intend
to release, since `2.0.0` is reserved for the first one. It costs nothing:
codes are spent and never reused, and the ceiling is 209 majors away.

One wrinkle, and it is not new. `version.mjs sync` with no argument stamps the
plain tree version, which takes the release slot, so a locally built install of
1.2.0 carries `10200999` while a CI develop build of the same 1.2.0 carries
`10200075`. The develop APK will not install over the local one. That is the
layout working as designed -- the release slot is the top of its version block
-- and a tree bump clears it, because the next block begins above the whole of
the last one.

**Restated 2026-08-28 for the counter in `c`.** The `-dev.N` codes above are no
longer produced; the shape of the argument is unchanged and the wrinkle is
unchanged with it, now between slot 0 and slot 999 of the same block:

```text
1.2.0   tree, before this amendment  -> 10200999
1.2.76  tree, release slot           -> 10276999
1.2.76  develop build, slot 0        -> 10276000
1.2.77  develop build, slot 0        -> 10277000
```

Every code the counter produces from here is above the `10200999` a phone is
carrying, so the bump installs over it with nothing to uninstall. A develop
build of 1.2.76 still will not install over a local build of 1.2.76, for the
same reason as before, and the next commit clears it.

### The dev counter is commits since the last release, not the CI run number

**Superseded 2026-08-28 by [What c counts from](#what-c-counts-from).** The
heading is kept so the links to it still resolve, and the section is kept
because the argument in it is still the argument: a counter must reset against
something, and a CI run number resets against nothing. What changed is what it
resets against. The counter is `c`, it counts from the commit that last wrote
the version into the tree, and the paragraph below about it never resetting is
the problem the fourth amendment exists to fix -- read it as the statement of a
defect, not as current behaviour. `version.mjs dev` no longer exists.

The obvious choice, `github.run_number`, is wrong twice over. It grows without
bound across the life of the repository, so it eventually reaches **999 and
collides with the reserved release slot** — producing a code Play Store has
already accepted — and then **1000, which falls outside the field entirely** and
makes every develop build fail.

Commits since the last release is monotonic within a release cycle and resets
at every release, which is exactly the property the field needs. Exceeding 998
commits in one cycle throws with a clear message rather than wrapping.

**With no release tag the counter has never reset, and a tree bump does not
reset it.** This is worth naming because it is easy to assume otherwise now
that the tree version moves. `lastReleaseTag()` lists
`release-v[0-9]*.[0-9]*.[0-9]*` and there is no such tag, so `releaseRange()`
falls back to `harmonix-final..HEAD` and `N` is every Sonduit commit there has
ever been. Checked: `version.mjs dev` prints `1.2.0-dev.75+c44bf47b` against
the 1.2.0 tree, with 75 commits in that range. Moving the tree version changes
the **base** of the string and nothing else; the counter is derived from the
range, and the range is anchored to a release tag, not to the version. So the
"release cycle" the 998 budget is measured against is currently the whole life
of the project -- 75 spent -- and only a `release-v` tag will reset it.

`N` also counts **commits, not builds**. It advances with every commit whether
or not anything is built, which is a faster rate than a per-tag build count.
See the [Consequences](#consequences).

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

That pre-tag bump is now the last of several rather than the only edit the
number ever sees, and it is the one that carries the release decision: for the
first release it is what reaches `2.0.0`, which accumulation deliberately does
not. See [When the tree version moves](#when-the-tree-version-moves).

There was briefly a `release-pr.yml` that computed the bump, drafted the
changelog and kept a `Release vX.Y.Z` pull request open from a `release/vX.Y.Z`
branch. It was removed. Automating the bump bought little -- it is one line in
one file -- and it paid for that with a branch and a pull request nobody asked
for appearing in the repository on every push to the integration branch. That
integration branch is gone too now, for the reason above; the workflow, the
`release/*` branches and the release pull requests went with it.

## Consequences

- The version is edited by hand in `Cargo.toml`: before a release tag, and
  whenever accumulated work warrants a bump. See
  [When the tree version moves](#when-the-tree-version-moves). Everything else
  is derived from it, and CI fails if the derived files disagree or if the tag
  disagrees with the file.
- **The number on screen now means something between releases.** That is the
  point of the amendment and it is the consequence that matters: a build's
  version moves with what went into it, instead of being frozen at the last
  release -- which here was none.
- **The budget was 998 commits and never reset; it is now 99 and it carries.**
  The third amendment recorded the defect: `devCounter` counted commits since
  the last *release* tag, there is no such tag, a mid-development bump did not
  reset it, and the count stood at 75 of 998, spent by ordinary commits rather
  than by deliberate build tags. The fourth amendment removes that counter
  entirely. `c` counts from the last bump, its ceiling is 99, and reaching it
  rolls into `b` instead of throwing. See [The rollover](#the-rollover).
- **`version.mjs next` is an answer again.** The third amendment left it a
  prompt because its range was "since the last release", so after a
  mid-development bump it re-counted commits that bump had already accounted
  for, and it read the two harmless breaking-change markers in that range as a
  major. Both follow from the range, and the range is now "since the last
  bump". The version is still a decision: `next` reports what the commits imply
  for `a` and `b`, and returns the version unchanged when they imply neither.
- **The number on screen now moves on its own.** `c` advances with every commit
  and `node tools/version.mjs bump` writes it. The version is 1.2.76 and its
  versionCode is 10276999.
- **A version that goes down cannot be installed over one that went up.** The
  versionCode follows the version, Android refuses a downgrade, and the fix is
  a manual uninstall on every device holding the higher build -- not a change
  to the formula. See
  [Lowering the version forces a reinstall on every phone](#lowering-the-version-forces-a-reinstall-on-every-phone).
- Patch releases have thin release notes by design. If that proves unhelpful,
  the change is one branch in `release.yml`.
