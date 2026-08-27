#!/usr/bin/env node
/**
 * The single source of truth for the version is `[workspace.package] version`
 * in the root Cargo.toml. Nothing else may declare one; Tauri and Gradle read
 * it from here.
 *
 * Commands:
 *   read                 print the workspace version, e.g. 0.1.0
 *   last-release         print the last Sonduit release tag, or nothing
 *   range                print the commit range since that tag
 *   next [range]         print the version the commits in the range imply
 *   dev [range]          print a develop build version
 *   code [version]       print the Android versionCode
 *   sync [version]       write the version into tauri.conf.json and gradle.properties
 *   check                verify every derived file agrees with Cargo.toml
 *
 * See docs/adr/ADR-008-versioning.md.
 */

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const CARGO_TOML = 'Cargo.toml';
const TAURI_CONF = 'desktop/src-tauri/tauri.conf.json';
const GRADLE_PROPERTIES = 'android/gradle.properties';

/**
 * ASCII record and unit separators, written as escapes so the raw control
 * bytes never end up in this file where an editor or a shell could mangle
 * them.
 */
const RECORD = '\u001e';
const UNIT = '\u001f';

/** Read `version = "x.y.z"` from the `[workspace.package]` table. */
export function readWorkspaceVersion(path = CARGO_TOML) {
  const text = readFileSync(path, 'utf8');
  const table = /\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/.exec(text);
  if (!table) throw new Error(`no [workspace.package] table in ${path}`);
  const match = /^\s*version\s*=\s*"([^"]+)"/m.exec(table[1]);
  if (!match) throw new Error(`no version in [workspace.package] of ${path}`);
  return match[1];
}

export function parseVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`not a plain semver release version: ${version}`);
  return { major: +match[1], minor: +match[2], patch: +match[3] };
}

/** Dev counter reserved to mean "this is the release, not a dev build". */
export const RELEASE_SLOT = 999;

/** Largest dev build counter, one below the release slot. */
export const MAX_DEV = RELEASE_SLOT - 1;

/**
 * Android's versionCode. Must increase monotonically forever: Play Store
 * rejects a code it has already seen, and the mistake is unrecoverable because
 * a version code can never be reused.
 *
 *   code = major * 10_000_000 + minor * 100_000 + patch * 1_000 + dev
 *
 * where `dev` is 0..998 for a develop build and 999 for a release.
 *
 * # Why the release takes the top slot
 *
 * The obvious layout, with a release at dev = 0, is NOT monotonic, and the
 * counterexample is one every project would hit:
 *
 *   1.3.0-dev.7   ->  ...007
 *   1.3.0         ->  ...000     <-- lower than its own dev builds
 *
 * A develop build named `1.3.0-dev.N` is a preview of the upcoming 1.3.0, so
 * it is published before it and its code must therefore be lower. Putting the
 * release at the top of each version's block is what makes that true.
 *
 * # Field widths, and why patch is capped at 99
 *
 * Each field must fit strictly inside the one above it, or a large value in a
 * low field outranks a bump in a high one:
 *
 *   dev    0..999      < 1_000      the patch multiplier
 *   patch  0..99       < 100        so patch*1_000 < 100_000, the minor multiplier
 *   minor  0..99       < 100        so minor*100_000 < 10_000_000, the major multiplier
 *
 * An earlier version allowed patch up to 999, which overflows:
 *
 *   1.0.999  ->  10_999_999
 *   1.1.0    ->  10_100_999     <-- lower, though it ships later
 *
 * and it also collided outright, 0.9.99 and 0.0.999 both mapping to 999_999.
 * That is the same Play-Store-unrecoverable failure this layout exists to
 * prevent, reintroduced one field down. Caught in review, not by the tests,
 * which only covered the worked examples from the ADR.
 *
 * # Ceiling
 *
 * Play Store caps versionCode at 2_100_000_000. With the fields above, the
 * largest code a given major can produce is
 * major*10_000_000 + 99*100_000 + 99*1_000 + 999 = major*10_000_000 + 9_999_999,
 * so major may go up to 209 (209 -> 2_099_999_999).
 */
export function versionCode({ major, minor, patch }, dev = RELEASE_SLOT) {
  if (major > 209) throw new Error(`major ${major} would exceed the Play Store versionCode cap`);
  if (minor > 99) throw new Error(`minor ${minor} exceeds the 99 the layout allows`);
  if (patch > 99) throw new Error(`patch ${patch} exceeds the 99 the layout allows`);
  if (dev < 0 || dev > RELEASE_SLOT) throw new Error(`dev counter ${dev} is outside 0..${RELEASE_SLOT}`);
  return major * 10_000_000 + minor * 100_000 + patch * 1_000 + dev;
}

/**
 * The dev counter for a develop build.
 *
 * Derived from the number of commits since the last release rather than from a
 * CI run number. A run number grows without bound across the life of the
 * repository, so it would eventually reach 999 and collide with the release
 * slot, and then 1000 and fall outside the field entirely. Commits since the
 * last release is monotonic within a release cycle and resets at every
 * release, which is exactly the property the field needs.
 *
 * @throws when a release cycle somehow exceeds {@link MAX_DEV} commits, which
 * is a real limit and must fail loudly rather than wrap into the release slot.
 */
export function devCounter(commitCount) {
  if (commitCount > MAX_DEV) {
    throw new Error(
      `${commitCount} commits since the last release exceeds the ${MAX_DEV} the ` +
        `versionCode dev field allows; cut a release before building again`,
    );
  }
  return commitCount;
}

/** Highest bump implied by a set of Conventional Commit subjects and bodies. */
export function bumpFromCommits(subjects, bodies = []) {
  let bump = 'patch';
  for (let i = 0; i < subjects.length; i += 1) {
    const subject = subjects[i];
    const body = bodies[i] ?? '';
    const header = /^([a-z]+)(?:\([a-z0-9-]+\))?(!)?: /.exec(subject);
    if (!header) continue;
    const [, type, bang] = header;

    if (bang || /^BREAKING CHANGE:/m.test(body)) return 'major';
    if (type === 'feat') bump = 'minor';
  }
  return bump;
}

export function applyBump(version, bump) {
  const { major, minor, patch } = parseVersion(version);
  if (bump === 'major') return `${major + 1}.0.0`;
  if (bump === 'minor') return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

/**
 * Split `git log` output formatted as `%s<UNIT>%b<RECORD>`.
 *
 * Exported so a test can drive it without needing a repository.
 */
export function splitCommitRecords(output) {
  const subjects = [];
  const bodies = [];

  for (const record of output.split(RECORD)) {
    // git separates records with a newline. Without stripping it, every
    // subject after the first begins with a line break, fails the
    // Conventional Commit pattern and is silently ignored, so a range full of
    // feat commits computes as a patch bump. This was a real bug, not a
    // hypothetical one.
    const cleaned = record.replace(/^\r?\n/, '');
    if (cleaned.trim() === '') continue;

    const at = cleaned.indexOf(UNIT);
    subjects.push(at === -1 ? cleaned : cleaned.slice(0, at));
    bodies.push(at === -1 ? '' : cleaned.slice(at + 1));
  }

  return { subjects, bodies };
}

/**
 * Read subjects and bodies for a range in ONE git call, so the two lists can
 * never drift out of alignment the way two separate calls would as soon as a
 * commit has an empty body.
 */
function commitsIn(range) {
  const output = execFileSync('git', ['log', `--format=%s${UNIT}%b${RECORD}`, range], {
    encoding: 'utf8',
  });
  return splitCommitRecords(output);
}

function shortSha() {
  return execFileSync('git', ['rev-parse', '--short=8', 'HEAD'], { encoding: 'utf8' }).trim();
}

/**
 * The most recent Sonduit release tag, or an empty string when there has not
 * been one.
 *
 * A plain `git describe --match 'v*'` is wrong here and would stay wrong: the
 * Harmonix tags v1.0.0 through v1.0.4 match that pattern too, and being
 * numerically higher than any early Sonduit version they would make the next
 * release compute as 1.1.0 instead of 0.2.0.
 *
 * Harmonix tags are exactly the tags reachable from `harmonix-final`, so they
 * are excluded by reachability rather than by guessing at version numbers.
 */
export function lastReleaseTag() {
  let tags;
  try {
    tags = execFileSync(
      'git',
      ['tag', '--list', 'v[0-9]*.[0-9]*.[0-9]*', '--merged', 'HEAD', '--sort=-v:refname'],
      { encoding: 'utf8' },
    )
      .split('\n')
      .map((tag) => tag.trim())
      .filter(Boolean);
  } catch {
    return '';
  }

  for (const tag of tags) {
    try {
      execFileSync('git', ['merge-base', '--is-ancestor', tag, 'harmonix-final'], {
        stdio: 'ignore',
      });
      // Exit 0 means the tag is reachable from the boundary, so it is Harmonix.
      continue;
    } catch {
      return tag;
    }
  }
  return '';
}

/** The commit range a version computation should consider. */
export function releaseRange() {
  const last = lastReleaseTag();
  return last ? `${last}..HEAD` : 'harmonix-final..HEAD';
}

function syncTauri(version) {
  if (!existsSync(TAURI_CONF)) return null;
  const config = JSON.parse(readFileSync(TAURI_CONF, 'utf8'));
  config.version = version;
  writeFileSync(TAURI_CONF, `${JSON.stringify(config, null, 2)}\n`);
  return { path: TAURI_CONF, after: version };
}

function syncGradle(version, code) {
  if (!existsSync(GRADLE_PROPERTIES)) return null;
  let text = readFileSync(GRADLE_PROPERTIES, 'utf8');
  const set = (key, value) => {
    const pattern = new RegExp(`^${key}=.*$`, 'm');
    text = pattern.test(text)
      ? text.replace(pattern, `${key}=${value}`)
      : `${text.replace(/\n?$/, '\n')}${key}=${value}\n`;
  };
  set('sonduitVersionName', version);
  set('sonduitVersionCode', code);
  writeFileSync(GRADLE_PROPERTIES, text);
  return { path: GRADLE_PROPERTIES, after: `${version} (${code})` };
}

/**
 * Rewrite the `version = "x.y.z"` on every path dependency in
 * `[workspace.dependencies]`.
 *
 * These exist because cargo-deny rejects a path-only dependency as a wildcard,
 * and cargo has no `version.workspace = true` for a dependency requirement. So
 * the number is written out, which means it is one more derived value that can
 * drift. It drifted the first time the workspace version changed: every crate
 * asked for ^0.1.0 while every crate was 1.0.4, and the whole workspace stopped
 * resolving.
 */
function syncWorkspaceDeps(version) {
  const text = readFileSync(CARGO_TOML, 'utf8');
  const updated = text.replace(
    /^(sonduit-[a-z-]+ = \{ path = "[^"]+", version = ")[^"]+(" \})$/gm,
    `$1${version}$2`,
  );
  if (updated === text) return null;
  writeFileSync(CARGO_TOML, updated);
  return { path: `${CARGO_TOML} [workspace.dependencies]`, after: version };
}

/** Every version requirement declared on a workspace path dependency. */
function workspaceDepVersions() {
  const text = readFileSync(CARGO_TOML, 'utf8');
  const found = [];
  const pattern = /^(sonduit-[a-z-]+) = \{ path = "[^"]+", version = "([^"]+)" \}$/gm;
  let match = pattern.exec(text);
  while (match) {
    found.push({ name: match[1], version: match[2] });
    match = pattern.exec(text);
  }
  return found;
}

function devCounterOf(version) {
  const match = /-dev\.(\d+)/.exec(version);
  if (!match) return RELEASE_SLOT;

  const counter = Number(match[1]);
  if (counter > MAX_DEV) {
    // RELEASE_SLOT is reserved. A dev build landing on it would produce
    // exactly the release code, and Play Store never forgets a code.
    throw new Error(
      `dev counter ${counter} would collide with the release slot; max is ${MAX_DEV}`,
    );
  }
  return counter;
}

function main() {
  const [command, ...args] = process.argv.slice(2);

  switch (command) {
    case 'read':
      console.log(readWorkspaceVersion());
      break;

    case 'last-release':
      console.log(lastReleaseTag());
      break;

    case 'range':
      console.log(releaseRange());
      break;

    case 'next': {
      const range = args[0] ?? releaseRange();
      const { subjects, bodies } = commitsIn(range);
      console.log(applyBump(readWorkspaceVersion(), bumpFromCommits(subjects, bodies)));
      break;
    }

    case 'dev': {
      // The counter is derived, not supplied: see devCounter for why a CI run
      // number cannot be used.
      const range = args[0] ?? releaseRange();
      const { subjects, bodies } = commitsIn(range);
      const next = applyBump(readWorkspaceVersion(), bumpFromCommits(subjects, bodies));
      console.log(`${next}-dev.${devCounter(subjects.length)}+${shortSha()}`);
      break;
    }

    case 'code': {
      const version = args[0] ?? readWorkspaceVersion();
      const base = version.split('-')[0];
      console.log(versionCode(parseVersion(base), devCounterOf(version)));
      break;
    }

    case 'sync': {
      const version = args[0] ?? readWorkspaceVersion();
      const base = version.split('-')[0];
      const code = versionCode(parseVersion(base), devCounterOf(version));
      for (const result of [
        syncTauri(version),
        syncGradle(version, code),
        syncWorkspaceDeps(base),
      ]) {
        if (result) console.log(`updated ${result.path} -> ${result.after}`);
      }
      break;
    }

    case 'check': {
      const version = readWorkspaceVersion();
      const problems = [];

      if (existsSync(TAURI_CONF)) {
        const config = JSON.parse(readFileSync(TAURI_CONF, 'utf8'));
        if (config.version !== version) {
          problems.push(`${TAURI_CONF} says ${config.version}, Cargo.toml says ${version}`);
        }
      }
      if (existsSync(GRADLE_PROPERTIES)) {
        const text = readFileSync(GRADLE_PROPERTIES, 'utf8');
        const name = /^sonduitVersionName=(.*)$/m.exec(text);
        if (name && name[1].trim() !== version) {
          problems.push(`${GRADLE_PROPERTIES} says ${name[1].trim()}, Cargo.toml says ${version}`);
        }
      }
      for (const dependency of workspaceDepVersions()) {
        if (dependency.version !== version) {
          problems.push(
            `[workspace.dependencies] ${dependency.name} requires ${dependency.version}, ` +
              `Cargo.toml says ${version}`,
          );
        }
      }

      if (problems.length > 0) {
        console.error('Version drift detected:');
        for (const problem of problems) console.error(`  - ${problem}`);
        console.error('\nRun: node tools/version.mjs sync');
        process.exit(1);
      }
      console.log(`OK: every derived file agrees with Cargo.toml (${version}).`);
      break;
    }

    default:
      console.error('Unknown command. See the header of tools/version.mjs.');
      process.exit(1);
  }
}

// Run only when invoked directly, so the pure functions above can be imported
// by a test without executing a command.
if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  main();
}
