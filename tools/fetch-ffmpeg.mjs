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
import { join, resolve } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { tmpdir } from 'node:os';

const TARGET_DIR = resolve('desktop/src-tauri/binaries');

/** Upstream builds, by platform. BtbN publishes both GPL and LGPL variants. */
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

/** Find the binary anywhere under `dir`. Archive layouts change between builds. */
function findBinary(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findBinary(path, name);
      if (found) return found;
    } else if (entry.name === name) {
      return path;
    }
  }
  return null;
}

const extracted = findBinary(work, source.binary);
if (!extracted) {
  console.error(`${source.binary} not found in the archive`);
  process.exit(1);
}

copyFileSync(extracted, destination);
rmSync(work, { recursive: true, force: true });

const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
console.log(`installed ${destination} (${size} MB)`);

// Prove it runs before declaring success.
const version = execFileSync(destination, ['-version'], { encoding: 'utf8' }).split('\n')[0];
console.log(version);
