#!/usr/bin/env node
/**
 * Renders desktop/src-tauri/binaries/THIRD-PARTY-LICENSES.txt from the crate
 * graph, so the installed application carries the attribution MIT, Apache-2.0
 * and the BSD licences require. See docs/licensing.md section 5.
 *
 * # Why it is generated and not committed
 *
 * The answer is a function of Cargo.lock. A committed copy would be stale the
 * first time a dependency moved and nothing would say so, which is the failure
 * mode that makes an attribution file worse than none. It is written into the
 * same gitignored directory as ffmpeg.exe and picked up by the same Tauri
 * `resources` glob, so it lands next to FFMPEG-LICENSE.txt on an installed
 * machine.
 *
 * about.toml decides which licences may appear, mirroring deny.toml.
 * tools/licences.hbs decides what the file looks like.
 *
 * Usage:
 *   node tools/gen-licences.mjs            generate
 *   node tools/gen-licences.mjs --check    report presence, generate nothing
 */

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const TARGET_DIR = resolve('desktop/src-tauri/binaries');
const CONFIG = 'tools/about.toml';
const TEMPLATE = 'tools/licences.hbs';

const destination = join(TARGET_DIR, 'THIRD-PARTY-LICENSES.txt');

const report = (path) => `${path} (${(statSync(path).size / 1024).toFixed(1)} KB)`;

if (process.argv.includes('--check')) {
  if (existsSync(destination)) {
    console.log(`present: ${report(destination)}`);
    process.exit(0);
  }
  console.log(`missing: ${destination}`);
  process.exit(1);
}

try {
  execFileSync('cargo', ['about', '--version'], { stdio: 'ignore' });
} catch {
  // `--features cli` is not optional: without it the crate builds and installs
  // no binary at all, and the only sign is a warning in the install output.
  console.error('cargo-about is not installed.');
  console.error('  cargo install cargo-about --locked --features cli');
  process.exit(1);
}

mkdirSync(TARGET_DIR, { recursive: true });

console.log(`generating ${destination}`);
try {
  execFileSync(
    'cargo',
    [
      'about',
      'generate',
      '--config',
      CONFIG,
      // Every feature, every shipped target: the notice has to cover the
      // Android build as well as the Windows one, and a crate that is only
      // reached behind a feature is still linked into something a user gets.
      '--all-features',
      '--workspace',
      // A crate whose licence cannot be resolved stops the build. The
      // alternative is a file that claims to be complete and silently is not.
      '--fail',
      '--output-file',
      destination,
      TEMPLATE,
    ],
    { stdio: 'inherit' },
  );
} catch {
  console.error('cargo-about failed; no licence file was produced.');
  process.exit(1);
}

if (!existsSync(destination) || statSync(destination).size === 0) {
  console.error(`cargo-about reported success but ${destination} is empty.`);
  process.exit(1);
}

console.log(`installed ${report(destination)}`);
