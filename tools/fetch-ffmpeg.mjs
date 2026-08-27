#!/usr/bin/env node
/**
 * Downloads an FFmpeg binary into desktop/src-tauri/binaries/ so the editor
 * screens work without asking the user to install anything.
 *
 * # Which build, and why it matters
 *
 * This fetches the **LGPL** build, not the GPL one. Sonduit is MIT. FFmpeg runs
 * as a separate process rather than being linked, so the licence boundary is a
 * process boundary either way, but shipping the LGPL build keeps the weaker
 * obligation and avoids arguing about the stronger one. See docs/licensing.md.
 *
 * It is also the **static** build, not the shared one, which is the opposite of
 * what the published archive sizes suggest. The shared archive is 64.6 MB
 * against the static 141.6 MB, but the archive is not what ships: the static
 * archive carries three whole programs and only ffmpeg.exe is copied out of it,
 * whereas the shared build's ffmpeg.exe is a 0.5 MB stub with load-time imports
 * on seven DLLs, every one of which has to travel with it. Measured on the
 * 2026-08-26 build:
 *
 *   static  ffmpeg.exe               115,361,792 B   110.0 MiB
 *   shared  ffmpeg.exe + 7 DLLs      134,812,672 B   128.6 MiB
 *
 * The shared build is 18.6 MiB larger installed, and 1.1 MiB larger again after
 * the LZMA the NSIS installer applies. Static wins on both counts.
 *
 * # Why this stays 110 MB
 *
 * The application asks FFmpeg for five encoders (aac, libmp3lame, pcm_s16le,
 * flac, libvorbis) and about a dozen audio filters. Nearly all of the 110 MB is
 * video code that is never reached. Reaching it needs a `configure` with those
 * codecs disabled, which means building FFmpeg from source, which means owning
 * a cross-compilation toolchain and writing a fresh LGPL compliance story for a
 * binary nobody else publishes. That trade is rejected. Every option that does
 * not need a toolchain was measured, and none of them pays:
 *
 *   shared build           128.6 MiB installed   larger, see above
 *   n8.1 release branch    108.7 MiB installed   1.2%, for pinning to an old branch
 *   installer compression  already LZMA          Tauri's NSIS default
 *   dropping ffprobe       already dropped       only ffmpeg.exe is copied here
 *
 * Note the last two: the 110 MB is what lands on disk after installation, not
 * what the user downloads. NSIS compresses it to about 35 MB in the installer.
 *
 * The binary is NOT committed. It is gitignored and fetched by CI, because a
 * 110 MB blob in git history is permanent and this one changes with every
 * upstream release.
 *
 * Usage:
 *   node tools/fetch-ffmpeg.mjs            download if missing
 *   node tools/fetch-ffmpeg.mjs --force    download even if present
 *   node tools/fetch-ffmpeg.mjs --check    report presence, download nothing
 */

import { createWriteStream, existsSync, mkdirSync, rmSync, statSync, readdirSync, copyFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { tmpdir } from 'node:os';

const TARGET_DIR = resolve('desktop/src-tauri/binaries');

/** Upstream builds, by platform. BtbN publishes GPL and LGPL, static and shared. */
const SOURCES = {
  win32: {
    url: 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip',
    archive: 'zip',
    binary: 'ffmpeg.exe',
  },
  linux: {
    url: 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-lgpl.tar.xz',
    archive: 'tar',
    binary: 'ffmpeg',
  },
};

const source = SOURCES[process.platform];
if (!source) {
  console.error(`No FFmpeg source configured for platform ${process.platform}.`);
  process.exit(1);
}

const destination = join(TARGET_DIR, source.binary);

if (process.argv.includes('--check')) {
  if (existsSync(destination)) {
    const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
    console.log(`present: ${destination} (${size} MB)`);
    process.exit(0);
  }
  console.log(`missing: ${destination}`);
  process.exit(1);
}

if (existsSync(destination) && !process.argv.includes('--force')) {
  console.log(`already present: ${destination}`);
  process.exit(0);
}

mkdirSync(TARGET_DIR, { recursive: true });

const work = join(tmpdir(), `sonduit-ffmpeg-${process.pid}`);
mkdirSync(work, { recursive: true });

const archivePath = join(work, source.archive === 'zip' ? 'ffmpeg.zip' : 'ffmpeg.tar.xz');

console.log(`downloading ${source.url}`);
const response = await fetch(source.url, { redirect: 'follow' });
if (!response.ok) {
  console.error(`download failed: ${response.status} ${response.statusText}`);
  process.exit(1);
}
await pipeline(Readable.fromWeb(response.body), createWriteStream(archivePath));
console.log(`downloaded ${(statSync(archivePath).size / 1024 / 1024).toFixed(1)} MB`);

console.log('extracting');
if (source.archive === 'zip') {
  // Not `tar`: on Windows the tar that ships with Git is GNU tar, which reads
  // a leading "C:" as a remote host and fails with "Cannot connect to C:".
  // PowerShell is always present on the platform this branch runs on.
  execFileSync(
    'powershell',
    [
      '-NoProfile',
      '-Command',
      `Expand-Archive -LiteralPath '${archivePath}' -DestinationPath '${work}' -Force`,
    ],
    { stdio: 'inherit' },
  );
} else {
  execFileSync('tar', ['-xJf', archivePath, '-C', work], { stdio: 'inherit' });
}

/** Find a file by name anywhere under `dir`. Archive layouts change between builds. */
function findFile(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findFile(path, name);
      if (found) return found;
    } else if (entry.name.toLowerCase() === name.toLowerCase()) {
      return path;
    }
  }
  return null;
}

const extracted = findFile(work, source.binary);
if (!extracted) {
  console.error(`${source.binary} not found in the archive`);
  process.exit(1);
}

copyFileSync(extracted, destination);

// The LGPL requires the licence text to travel with the binary, so the build
// fails without it rather than shipping something that cannot legally be
// distributed. BtbN includes it in the archive; taking it from there rather
// than committing a copy means it always matches the build being shipped.
const licence = findFile(work, 'LICENSE.txt') ?? findFile(work, 'COPYING.LGPLv2.1');
if (!licence) {
  console.error('no licence text in the archive; refusing to ship an unlicensed binary');
  rmSync(work, { recursive: true, force: true });
  process.exit(1);
}
copyFileSync(licence, join(dirname(destination), 'FFMPEG-LICENSE.txt'));
console.log(`installed ${join(dirname(destination), 'FFMPEG-LICENSE.txt')}`);

rmSync(work, { recursive: true, force: true });

const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
console.log(`installed ${destination} (${size} MB)`);

// Prove it runs before declaring success.
const version = execFileSync(destination, ['-version'], { encoding: 'utf8' }).split('\n')[0];
console.log(version);
