# Licensing

What Sonduit takes from where, under which licence, and which decisions carry
a risk of forcing the project to GPL.

Sonduit itself is **MIT licensed** (see `LICENSE` at the repository root).
Every decision below is measured against the goal of keeping it that way.

> This is an engineering analysis, not legal advice. The GPL boundary in
> section 3 is the one place where being wrong is expensive, so it is written
> conservatively: where there is doubt, the answer is "do not".

---

## 1. Summary table

| Component | Origin | Licence | How Sonduit uses it | Risk |
| --- | --- | --- | --- | --- |
| Wire protocol description | Scream driver source | MS-PL | Read to write [protocol.md](./protocol.md). Facts about bytes, not code. | None |
| Scream driver binary | `duncanthrax/scream` `Install/driver/` | MS-PL | **Nothing shipped yet.** See ADR-002 | Low when it ships, conditions apply |
| Scream driver source | `duncanthrax/scream` `Scream/` | MS-PL | Read only. Not vendored, not compiled. | None while unmodified |
| `martinellimarco/scream-android` | GitHub | **GPL-3.0** | **Nothing. Not read for implementation, not copied, not adapted.** | **High if violated** |
| `BreadFish64/AndroidUsbAudioDevice` | GitHub | **No licence at all** | **Nothing. Read the README only.** | **High if violated** |
| **FFmpeg** | BtbN builds, **LGPL variant** | LGPL-2.1+ | Bundled binary, run as a **separate process**. Never linked. | Low, notice required |
| Rust crates | crates.io | MIT / Apache-2.0 | Linked normally | None |
| Oboe | `google/oboe` | Apache-2.0 | Linked on Android | None, notice required |

---

## 2. MS-PL, and what redistributing the Scream driver actually requires

The Scream driver is under the Microsoft Public License. It is a permissive,
file-level-copyleft licence: it permits derivative works and commercial
distribution, and it does **not** reach across process or module boundaries
the way the GPL does.

**The reference checkout is pinned to
`d789743c248b11d1df7e5ecc546b1bc60b90cd91` (2026-08-11).** A floating reference
would not do: the protocol facts in [protocol.md](./protocol.md) were read out
of a specific state of that tree, and if upstream changes the wire format, an
unpinned note would make it look as though this project had described the new
one. Anything read from a later commit has to be recorded as such.

The obligations that bind Sonduit, quoted from the licence text in
`third_party/reference/scream/LICENSE`:

- **3(C)** — "If you distribute any portion of the software, you must retain
  all copyright, patent, trademark, and attribution notices that are present
  in the software."
- **3(D)** — "If you distribute any portion of the software in source code
  form, you may do so only under this license... If you distribute any portion
  of the software in compiled or object code form, you may only do so under a
  license that complies with this license."
- **3(A)** — No trademark licence. Sonduit must not present itself as Scream
  or use the name as branding.

What this means concretely:

1. Shipping the prebuilt `Scream.sys` / `Scream.cat` / `Scream.inf` inside
   `driver/` would be permitted, provided the MS-PL text and the copyright
   notice travel with them. **`driver/` is empty today and ships nothing**, so
   no such obligation is live yet. When it does ship, it must carry its own
   `LICENSE` copy and a `NOTICE` naming the upstream project and commit.
   Separately, ADR-002 found the upstream binaries are unusable anyway.
2. MS-PL 3(D) applies **to the driver files only**. It does not license
   Sonduit's own code, because MS-PL is a per-file copyleft, not a work-wide
   one. Sonduit's MIT licence is unaffected.
3. MIT is a licence that "complies with" MS-PL for the purposes of 3(D) in the
   sense that it does not add restrictions the MS-PL forbids — but rather than
   relitigate that, **the driver files stay under MS-PL and are not
   relicensed**. Two licences, clearly separated by directory. That removes
   the question entirely.

### 2.1 Why the driver must not be modified

This is a signing constraint, not a licence one, but it lands in the same
decision. See ADR-002 for the full argument and
[research/wasapi-vs-virtual-driver.md](./research/wasapi-vs-virtual-driver.md)
for the evidence. In short: the upstream binaries are already signed. The
moment a single byte changes, that signature is void and Sonduit is in the
business of getting a kernel driver signed, which is a different and far more
expensive project.

**MS-PL permits modification. Windows makes it impractical.** Keeping the
driver bit-identical is therefore a hard project rule, and any proposal to
patch it must go through a new ADR.

---

## 2.2 FFmpeg, and why a subprocess is the licence boundary

The editor screens convert, master, trim and modify audio. That work is done by
**FFmpeg**, fetched by `tools/fetch-ffmpeg.mjs` into
`desktop/src-tauri/binaries/` and bundled as a Tauri resource.

Two decisions keep this clean:

1. **The LGPL build, not the GPL one.** BtbN publishes both. The LGPL variant
   omits the GPL-only codecs and carries the weaker obligation, so there is no
   argument to have.
2. **It runs as a separate process.** Sonduit spawns `ffmpeg.exe` and reads its
   exit status; it does not link against any FFmpeg library. Copyleft reaches
   across a linking boundary, not across a process boundary, and that is what
   lets an MIT application ship it.

Obligations that follow, none of which touch this project's own licence:

- Ship the LGPL text alongside the binary and state that FFmpeg is included.
- Do not modify the binary. It is used exactly as published.
- Make the corresponding source available. Upstream already does; a link to the
  exact build satisfies this.

**The binary is never committed.** It is 110 MB, changes with every upstream
release, and git history is permanent. `.gitignore` excludes
`desktop/src-tauri/binaries/`, and CI fetches it before building.

Both obligations are met. `tools/fetch-ffmpeg.mjs` copies the licence text out
of the same archive the binary came from, into
`desktop/src-tauri/binaries/FFMPEG-LICENSE.txt`, which Tauri bundles alongside
`ffmpeg.exe`. Taking it from the archive rather than committing a copy means it
always matches the build being shipped, and the fetch fails outright if the
archive has no licence in it rather than producing something that cannot
legally be distributed.

The About screen lists FFmpeg, its licence, the fact that it is unmodified,
where it came from, and where the full text is installed.

---

## 3. The GPL boundary

### 3.1 `scream-android` is GPL-3.0 and is quarantined

`martinellimarco/scream-android` is licensed GPL-3.0
(`third_party/reference/scream-android/LICENSE`, 674 lines, the standard
GPLv3 text).

GPL-3.0 is a strong copyleft. If Sonduit's Android receiver were a derivative
work of it, **the whole of Sonduit's Android application would have to be
released under GPL-3.0**, and MIT distribution of that component would no
longer be possible.

Therefore:

- **No file from that repository is copied, in whole or in part.**
- **No file from that repository is translated, transliterated, or adapted**
  into Kotlin or Rust. Rewriting GPL code in another language produces a
  derivative work; changing the language does not launder the copyright.
- **Its source is not consulted while implementing Sonduit's receiver.**
- It is not a dependency, is not linked, and is not vendored.

The clone under `third_party/reference/` exists so this document could
establish its licence. `third_party/reference/` is in `.gitignore`, so no
GPL-licensed byte is ever committed to this repository or swept up by a build,
a lint, or a release artifact.

### 3.2 Why reading the *driver* for the protocol is a different act

The protocol in [protocol.md](./protocol.md) was derived from the **MS-PL
driver**, which is the sender. That was a deliberate choice, and it is worth
being precise about why it is sound on two independent grounds:

1. **Licence.** MS-PL permits reading, deriving from, and building on the
   source outright. There was never a restriction to work around.
2. **Subject matter.** A wire format is a functional interface — a fact about
   which bytes appear on a network in which order. Facts and interfaces are
   not the protected expression of a program. Documenting them by observing a
   sender is the same activity as documenting them with a packet capture.

Ground 1 alone is sufficient. Ground 2 means the conclusion would hold even
if the sender's licence had been restrictive.

**Sonduit's receiver is a clean-room implementation** written against
`protocol.md`, which cites only MS-PL sources.

### 3.3 `AndroidUsbAudioDevice` has no licence, which is worse than GPL

`BreadFish64/AndroidUsbAudioDevice` contains **no `LICENSE`, `COPYING`, or
equivalent file**, and its README states no licence terms. Verified by
searching the tree.

Absent an explicit grant, the work is under exclusive copyright — "all rights
reserved" by default. That is **more restrictive than GPL-3.0**: GPL at least
grants rights subject to conditions, whereas this grants nothing at all.

Consequences:

- **No code from it may be copied or adapted under any circumstances.**
- Reading its **README** for a description of the approach is fine: the
  README describes an idea (drive the Android USB accessory protocol from a
  PC and pipe audio over a bulk endpoint), and ideas are not copyrightable.
  The prose itself is not reproduced here beyond short factual attribution.
- Its *source* is treated exactly like `scream-android`: quarantined.

The idea it documents — USB Accessory / AOA as a transport — is evaluated on
its merits in
[research/usb-transport.md](./research/usb-transport.md). Nothing about that
evaluation depends on its code.

---

## 4. Decisions that would force Sonduit to GPL

Flagged explicitly, as required. **If any of these is ever proposed, stop and
escalate before writing code.**

| Proposed action | Effect | Verdict |
| --- | --- | --- |
| Copy or adapt any part of `scream-android` | Android app becomes a GPL-3.0 derivative work | **Forbidden** |
| Link a GPL-3.0 library into the Android app or the shared core | Whole linked work becomes GPL-3.0 | **Forbidden without a new ADR** |
| Vendor GPL code into `crates/` | Contaminates the shared core, which also ships on Windows — would pull the desktop app in too | **Forbidden** |
| Add a GPL-licensed Rust crate as a dependency | Same as above. Note most of the Rust ecosystem is MIT/Apache-2.0, so this is unlikely by accident, but `cargo deny` should enforce it | **Forbidden**, to be enforced mechanically |
| Copy any code from `AndroidUsbAudioDevice` | Straight copyright infringement, no licence at all | **Forbidden** |
| Modify and redistribute the Scream driver | Permitted by MS-PL, but voids the signature. Separate and expensive problem | **Needs a new ADR** |
| Use the Scream name or logo in Sonduit's branding | MS-PL 3(A) grants no trademark rights | **Forbidden** |

The shared-core point deserves emphasis. Because `sonduit-core` is compiled
into **both** the Windows binary and the Android app, a copyleft dependency
anywhere in it contaminates the desktop product as well as the mobile one.
The core's dependency list is the highest-value thing to keep clean, which is
one more reason for the "no platform dependencies, few carefully chosen
crates" rule in ADR-001.

### 4.1 Mechanical enforcement

Discipline is not a control. Both mechanisms are in place:

- **`cargo deny`** with a permissive-only licence allowlist, denying everything
  else, run by the `licences` job in `.github/workflows/ci.yml`. The list lives
  in `deny.toml` and is `MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`,
  `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `CC0-1.0`,
  `Unlicense` and `BSL-1.0`. **MPL-2.0 is deliberately excluded**: it is
  file-level copyleft and does not meet the permissive-only rule.
- **`third_party/reference/` in `.gitignore`**, so quarantined source cannot be
  committed by accident.

---

## 5. Attribution Sonduit owes

Shipped with the distributed application:

- **Scream driver** (`driver/`): MS-PL text, Microsoft's 2015 copyright
  notice, and attribution to the `duncanthrax/scream` project with the exact
  commit the binaries came from.
- **Oboe** (Android): Apache-2.0 requires the licence text and any `NOTICE`
  file to be reproduced.
- **Rust dependencies**: MIT and Apache-2.0 both require the copyright notice
  and licence text. A generated third-party licence file
  (`cargo about` or equivalent) should be bundled and surfaced in the About
  screen. Not yet implemented; tracked in the roadmap.

Sonduit currently ships no such aggregated notice file. That is a real gap
and is listed in [roadmap.md](./roadmap.md).

---

## 6. Not verified

- Whether the prebuilt binaries in the Scream repository's `Install/driver/`
  were built from the source in the same repository. There is no reproducible
  build, and the binaries are not checksummed against a build output. Sonduit
  redistributes what upstream published, and says so.
- Whether Microsoft's signature on those binaries remains valid for new
  installations on current Windows builds. This depends on cross-signing
  policy and is treated in
  [research/wasapi-vs-virtual-driver.md](./research/wasapi-vs-virtual-driver.md).
- The exact upstream commit of the driver binaries. To be pinned when the
  driver is actually vendored into `driver/`; a `NOTICE` with a floating
  reference is not good enough.
