# Development environment

What this machine can build, what it cannot, and therefore what cannot be
verified locally. Recorded before the architecture work so that ADRs state
honestly which of their claims rest on evidence and which rest on reading.

Probed on **2026-08-27**.

---

## 1. Host

| Property | Value |
| --- | --- |
| OS | Windows 11 Pro, build 10.0.26200 |
| Architecture | x86_64 |
| Shell | PowerShell 5.1, plus Git Bash |
| Free space on the working volume | 815 GB |
| Network | Available (clones and package installs succeeded) |

Being on Windows is the single most useful fact here: it means the WASAPI half
of the project has a real local feedback loop. The Android half does not.

---

## 2. Toolchain inventory

### 2.1 Present and working

| Tool | Version | Verified how |
| --- | --- | --- |
| rustc | 1.97.1 (8bab26f4f 2026-07-14) | `rustc --version` |
| cargo | 1.97.1 (c980f4866 2026-06-30) | `cargo --version` |
| rustfmt | installed | `cargo fmt --check` run clean |
| clippy | installed | `cargo clippy -- -D warnings` run clean |
| Node.js | v24.18.0 | `node -v` |
| npm | 12.0.2 | `npm -v` |
| Tauri CLI | 2.9.4 | `npx tauri --version` |
| MSVC | Visual Studio Build Tools 2026, 18.8.12009.203 | `vswhere` |
| Windows SDK | 10.0.26100.0 and 10.0.28000.0 | `Windows Kits\10\Include` |
| git | 2.55.0.windows.3 | `git --version` |
| GitHub CLI | 2.96.0 | `gh --version` |

Installed Rust targets: **`x86_64-pc-windows-msvc` only.**

### 2.2 Absent

| Missing | Consequence |
| --- | --- |
| **WDK** (no `km` headers under `Windows Kits\10\Include\*\km`) | A kernel-mode driver cannot be compiled here. Only the SDK user-mode headers are present. |
| **JDK** (`java` not on PATH) | Gradle cannot run. |
| **Android SDK** (`ANDROID_HOME` and `ANDROID_SDK_ROOT` both unset) | No `adb`, no platform tools, no emulator. |
| **Android NDK** (`ANDROID_NDK_HOME` unset) | No native cross-compilation. |
| **`cargo-ndk`** (`cargo ndk` reports "no such command") | The documented Android build path is unavailable. |
| Android targets in rustup (`aarch64-linux-android` etc.) | Not installed. |
| **A physical Android device or an emulator with working audio** | Nothing can be listened to. |

---

## 3. What can be verified locally

- `sonduit-core` — fully. It is deliberately free of platform dependencies
  (ADR-001), so it compiles and unit-tests on this host like any ordinary
  library. **This is the main reason that constraint exists.**
- `sonduit-transport` — UDP send and receive over loopback and over the LAN.
- `sonduit-capture-win` — compiles and runs. WASAPI is user-mode and the MSVC
  toolchain plus Windows SDK are present.
- `sonduit-desktop` (Tauri) — compiles, bundles, and **runs**. Verified: the
  application was built with `--features custom-protocol`, launched, and
  screenshotted; the acrylic backdrop, custom window chrome, navigation,
  theme switching and the custom dropdown all render.
- The frontend — `npm run build` succeeds.
- `cargo fmt`, `cargo clippy -D warnings`, `cargo test`.

## 4. What cannot be verified locally

| Cannot verify | Why | Where it gets verified instead |
| --- | --- | --- |
| Android build of any kind | No JDK, SDK, NDK or `cargo-ndk` | GitHub Actions `ubuntu-latest` |
| `sonduit-playback-android` | Same | CI compile only; behaviour needs hardware |
| UniFFI binding generation and the Kotlin side | Same | CI |
| **Anything audible, end to end** | No Android device, no emulator with audio | **Nowhere. Requires the maintainer on real hardware.** |
| Real measured latency | Same | Same |
| AAudio EXCLUSIVE / LOW_LATENCY behaviour, burst sizes, OEM quirks | Same | Same |
| Kernel driver compilation | No WDK | Not planned — the driver is redistributed unmodified (ADR-002) |
| Driver installation and Windows endpoint enumeration | Needs an elevated install of an unsigned-by-us driver | Manual, on the maintainer's machine |
| USB tethering and `adb` paths | No Android device | Manual |
| MSI and NSIS bundle installation | Bundling is testable; installing and uninstalling cleanly is not automated | CI builds the artifacts; installation is manual |

---

## 5. The consequence for the walking skeleton

Part 5 of the brief asks for a walking skeleton that produces sound. **On this
machine that requirement cannot be met, and pretending otherwise would be
worse than saying so.**

The skeleton therefore ends in a **`FileSink` that writes a WAV file**, and
the test asserts on the file's contents: header fields, sample count, and that
the decoded samples match the 440 Hz sine that was fed in, within tolerance.
That is a real, checkable end-to-end assertion over the whole chain —
source, core, encode, UDP, decode, jitter buffer, sink — and it runs in CI.

What it does **not** prove:

> **End-to-end audibility is unverified.** No audio has been played through
> an Android device. Latency figures in
> [latency-budget.md](./latency-budget.md) are budgets derived from
> documentation and arithmetic, **not measurements**. They must be validated
> on real hardware before any of them is quoted as a result.

---

## 6. To reproduce this environment for full development

Not needed to work on `sonduit-core`. Needed for the Android half:

1. **JDK 17** (the version Android Gradle Plugin currently expects).
2. **Android SDK**, platform 34 or newer, plus platform-tools for `adb`.
3. **Android NDK** r26 or newer; set `ANDROID_NDK_HOME`.
4. `cargo install cargo-ndk`
5. `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`
6. A physical Android device. An emulator is enough to prove the app starts;
   it is **not** enough to judge latency, and its audio path is not
   representative.

For the driver side:

7. Administrator rights to install the Scream driver from `driver/`.
8. The WDK is **not** required, because Sonduit does not build the driver.

---

## 7. Not verified

- Whether the two Windows SDK versions present (26100, 28000) both work for
  the WASAPI code. Only the default selection has been exercised.
- Whether `ANDROID_HOME` is genuinely unset versus set only in a GUI session
  that this shell does not inherit. The probe read the environment of the
  shell; a system-level variable set after the shell started would not appear.
- Windows display scaling was not recorded, which matters when interpreting
  the screenshots taken during UI verification.
