#!/usr/bin/env node
/**
 * The single source of truth for the version is `[workspace.package] version`
 * in the root Cargo.toml. Nothing else may declare one; Tauri and Gradle read
 * it from here.
 *
 * The version is `a.b.c`: `a` major, `b` minor, `c` the build counter. `c` is
 * not a patch number and is not a decision -- it advances by one per commit.
 * `a` and `b` are decisions. See docs/adr/ADR-008-versioning.md.
 *
 * Commands:
 *   read                 print the workspace version, e.g. 1.2.76
 *   anchor               print the range `c` is counted over, and what it implies
 *   advance              print the version HEAD implies, counter advanced
 *   bump [what]          write that version and sync; `what` is nothing (just
 *                        advance the counter), `minor`, `major`, or a version
 *   next                 print the version a release cut here would carry
 *   range                print the commit range since the last release
 *   code [version] [--develop]   print the Android versionCode
 *   sync [version] [--develop]   write the version into every derived file
 *   check                verify every derived file agrees with Cargo.toml
 */

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const CARGO_TOML = 'Cargo.toml';
const TAURI_CONF = 'desktop/src-tauri/tauri.conf.json';
const GRADLE_PROPERTIES = 'android/gradle.properties';
const README = 'README.md';

/**
 * ASCII record and unit separators, written as escapes so the raw control
 * bytes never end up in this file where an editor or a shell could mangle
 * them.
 */
const RECORD = '\u001e';
const UNIT = '\u001f';

/** Read `version = "x.y.z"` from the `[workspace.package]` table of a file. */
export function parseWorkspaceVersion(text, label = CARGO_TOML) {
  const table = /\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/.exec(text);
  if (!table) throw new Error(`no [workspace.package] table in ${label}`);
  const match = /^\s*version\s*=\s*"([^"]+)"/m.exec(table[1]);
  if (!match) throw new Error(`no version in [workspace.package] of ${label}`);
  return match[1];
}

export function readWorkspaceVersion(path = CARGO_TOML) {
  return parseWorkspaceVersion(readFileSync(path, 'utf8'), path);
}

/**
 * Rewrite `[workspace.package] version`.
 *
 * Spliced by index rather than by a global replace: `version = "..."` appears
 * on every workspace path dependency too, and a careless pattern would rewrite
 * those here as well as in {@link rewriteWorkspaceDeps}, which is how the two
 * could disagree about what they had written.
 */
export function rewriteWorkspaceVersion(text, version) {
  parseVersion(version);
  const heading = '[workspace.package]';
  const table = /\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/.exec(text);
  if (!table) throw new Error(`no [workspace.package] table in ${CARGO_TOML}`);
  const line = /^([ \t]*version\s*=\s*")([^"]+)(")/m.exec(table[1]);
  if (!line) throw new Error(`no version in [workspace.package] of ${CARGO_TOML}`);

  const at = table.index + heading.length + line.index;
  return `${text.slice(0, at)}${line[1]}${version}${line[3]}${text.slice(at + line[0].length)}`;
}

export function parseVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`not a plain a.b.c version: ${version}`);
  return { major: +match[1], minor: +match[2], patch: +match[3] };
}

/**
 * The two build-kind slots in the low field of the versionCode.
 *
 * They used to be a range: 0..998 held the develop build counter and 999 meant
 * "this is the release". The counter moved into `c` -- the version itself --
 * so nothing counts here any more, and what is left is the one distinction the
 * version string no longer makes: a develop build of a.b.c versus the release
 * of the same a.b.c. The release keeps the top of the block so it outranks the
 * develop build it follows.
 *
 * 1..998 stay unused rather than being reclaimed. Narrowing the field would
 * divide every code above it, and codes already installed on real phones -- and
 * a code, once accepted, is never reusable -- must stay below the ones this
 * still has to produce. See ADR-008.
 */
export const RELEASE_SLOT = 999;
export const DEVELOP_SLOT = 0;

/** Largest build counter `c` may reach before it rolls into `b`. */
export const MAX_COUNTER = 99;

/**
 * Android's versionCode. Must increase monotonically forever: Play Store
 * rejects a code it has already seen, and the mistake is unrecoverable because
 * a version code can never be reused.
 *
 *   code = major * 10_000_000 + minor * 100_000 + patch * 1_000 + slot
 *
 * where `slot` is {@link DEVELOP_SLOT} or {@link RELEASE_SLOT}.
 *
 * # Why the release takes the top slot
 *
 * The obvious layout, with a release at slot 0, is NOT monotonic when a
 * develop build of the same version was cut first:
 *
 *   1.3.0 develop  ->  ...000
 *   1.3.0 release  ->  ...000     <-- cannot be told apart, or worse, is lower
 *
 * A develop build of a.b.c is published before the release of a.b.c, so its
 * code must be lower. Putting the release at the top of each version's block is
 * what makes that true.
 *
 * # Field widths, and why the counter is capped at 99
 *
 * Each field must fit strictly inside the one above it, or a large value in a
 * low field outranks a bump in a high one:
 *
 *   slot   0..999      < 1_000      the patch multiplier
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
 * That cap is now load-bearing twice over: it is also why the build counter
 * rolls into the minor at 100 instead of growing. See {@link advanceVersion}.
 *
 * # Ceiling
 *
 * Play Store caps versionCode at 2_100_000_000. With the fields above, the
 * largest code a given major can produce is
 * major*10_000_000 + 99*100_000 + 99*1_000 + 999 = major*10_000_000 + 9_999_999,
 * so major may go up to 209 (209 -> 2_099_999_999).
 */
export function versionCode({ major, minor, patch }, slot = RELEASE_SLOT) {
  if (major > 209) throw new Error(`major ${major} would exceed the Play Store versionCode cap`);
  if (minor > 99) throw new Error(`minor ${minor} exceeds the 99 the layout allows`);
  if (patch > MAX_COUNTER) throw new Error(`patch ${patch} exceeds the 99 the layout allows`);
  if (slot < 0 || slot > RELEASE_SLOT) throw new Error(`slot ${slot} is outside 0..${RELEASE_SLOT}`);
  return major * 10_000_000 + minor * 100_000 + patch * 1_000 + slot;
}

/**
 * The version `commits` further on from `base`.
 *
 * `c` is the build counter and advances by one per commit. It cannot grow past
 * 99, because `patch*1_000` has to stay inside the minor field of the
 * versionCode -- see {@link versionCode} -- so it **rolls into `b`**:
 * 1.2.99 followed by one commit is 1.3.0.
 *
 * The alternative, throwing at 99 and demanding a manual minor bump, was
 * rejected: it stops every build for a limit that is reached by ordinary work,
 * roughly every hundred commits, and the fix it demands is the same one this
 * carry performs. A carry that happens on its own is also what makes the
 * counter's anchor correct, because the bump that records the roll is the
 * commit the next hundred are counted from.
 *
 * The roll is a carry, not a reset: 100 commits past 1.2.0 is 1.3.0 and 101 is
 * 1.3.1, so the version keeps advancing by one per commit across the boundary
 * whether or not anyone bumped the tree in between.
 */
export function advanceVersion(base, commits) {
  if (!Number.isInteger(commits) || commits < 0) {
    throw new Error(`commit count must be a non-negative integer, got ${commits}`);
  }
  const { major, minor, patch } = parseVersion(base);
  const raw = patch + commits;
  const width = MAX_COUNTER + 1;
  const rolledMinor = minor + Math.floor(raw / width);
  if (rolledMinor > 99) {
    throw new Error(
      `the counter rolling ${Math.floor(raw / width)} times would put the minor at ` +
        `${rolledMinor}, past the 99 the versionCode layout allows; raise the major`,
    );
  }
  return `${major}.${rolledMinor}.${raw % width}`;
}

/**
 * The commit the counter is anchored to: the one that last wrote the version
 * now in the tree.
 *
 * `entries` are the commits touching Cargo.toml, newest first, each with the
 * version its Cargo.toml carried. The anchor is the oldest commit of the
 * newest unbroken run carrying `version`; `null` means no committed Cargo.toml
 * carries it, which is a bump sitting in the working tree, and the caller
 * should count from HEAD.
 *
 * Anchoring to the last **bump** rather than to the last **release** is what
 * makes the roll in {@link advanceVersion} work. Counting from a release tag
 * cannot reset -- there has never been a release tag here -- so a version that
 * rolled to 1.3.0 would be handed the same overflowing count again on the next
 * commit and roll to 1.4.0, and again, and again. The version records its own
 * anchor: the commit that wrote a.b.c is where c was last true.
 */
export function findAnchor(entries, version) {
  let anchor = null;
  for (const entry of entries) {
    if (entry.version !== version) break;
    anchor = entry.sha;
  }
  return anchor;
}

/**
 * The version a release cut at `version` would carry.
 *
 * `patch` returns the version unchanged, and that is not an oversight: `c` is
 * the build counter and has already moved with every commit, so there is no
 * third number left to bump. A release of fixes is the tag on the version the
 * counter has already produced.
 */
export function nextRelease(version, bump) {
  const { major, minor } = parseVersion(version);
  if (bump === 'major') return `${major + 1}.0.0`;
  if (bump === 'minor') return `${major}.${minor + 1}.0`;
  return version;
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
      ['tag', '--list', 'release-v[0-9]*.[0-9]*.[0-9]*', '--merged', 'HEAD', '--sort=-v:refname'],
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

/**
 * The commit range a changelog or a commit-message check should consider.
 *
 * This is the **release** range, and it is no longer what the build counter is
 * measured over: see {@link findAnchor}.
 */
export function releaseRange() {
  const last = lastReleaseTag();
  return last ? `${last}..HEAD` : 'harmonix-final..HEAD';
}

/**
 * Every commit that touched Cargo.toml, newest first, with the workspace
 * version it carried, stopping as soon as one differs from `version`.
 *
 * The scan is bounded because that is all {@link findAnchor} reads, and because
 * the far end of this history is Harmonix, whose Cargo.toml has no workspace
 * table at all.
 */
function cargoVersionHistory(version, limit = 50) {
  const shas = execFileSync(
    'git',
    ['log', `--max-count=${limit}`, '--format=%H', 'HEAD', '--', CARGO_TOML],
    { encoding: 'utf8' },
  )
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);

  const entries = [];
  for (const sha of shas) {
    let carried = null;
    try {
      carried = parseWorkspaceVersion(
        execFileSync('git', ['show', `${sha}:${CARGO_TOML}`], { encoding: 'utf8' }),
        `${sha}:${CARGO_TOML}`,
      );
    } catch {
      // Unreadable or pre-workspace: it cannot be the anchor, and neither can
      // anything older, so the run ends here either way.
    }
    entries.push({ sha, version: carried });
    if (carried !== version) break;
  }
  return entries;
}

/** The anchor commit for the version in the tree, and the commits since it. */
export function counterState(version = readWorkspaceVersion()) {
  const anchor = findAnchor(cargoVersionHistory(version), version);
  if (!anchor) {
    // Nothing committed carries this version: it was just written into the
    // working tree, so this is the anchor and nothing has happened since.
    return { anchor: null, commits: 0 };
  }
  const commits = Number(
    execFileSync('git', ['rev-list', '--count', `${anchor}..HEAD`], { encoding: 'utf8' }).trim(),
  );
  return { anchor, commits };
}

function syncTauri(version) {
  if (!existsSync(TAURI_CONF)) return null;
  const config = JSON.parse(readFileSync(TAURI_CONF, 'utf8'));
  config.version = version;
  writeFileSync(TAURI_CONF, `${JSON.stringify(config, null, 2)}\n`);
  return { path: TAURI_CONF, after: version };
}

/**
 * Rewrite the version shown on the README badge.
 *
 * A number written by hand in a readme is wrong within a release or two, and
 * this one is on the first screen anybody sees.
 */
function syncReadme(version) {
  if (!existsSync(README)) return null;
  const text = readFileSync(README, 'utf8');
  const updated = text.replace(
    /(!\[Version\]\(https:\/\/img\.shields\.io\/badge\/version-)[^-]+(-)/,
    `$1${version}$2`,
  );
  if (updated === text) return null;
  writeFileSync(README, updated);
  return { path: README, after: version };
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
export function rewriteWorkspaceDeps(text, version) {
  return text.replace(
    /^(sonduit-[a-z-]+ = \{ path = "[^"]+", version = ")[^"]+(" \})$/gm,
    `$1${version}$2`,
  );
}

function syncWorkspaceDeps(version) {
  const text = readFileSync(CARGO_TOML, 'utf8');
  const updated = rewriteWorkspaceDeps(text, version);
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

/**
 * Stamp every derived file.
 *
 * The tree always carries the release slot. Only a develop build asks for
 * {@link DEVELOP_SLOT}, and it does so explicitly, because the version string
 * no longer says which kind of build it is -- the tag does.
 *
 * All four files take the same number. The README badge and the workspace
 * dependency requirements used to be forced back to the tree version instead,
 * because a develop build's version was a different string -- `1.1.0-dev.54`
 * -- and stamping it here would have made every crate require a version no
 * crate had. There is no such string any more: a develop build and the tree
 * are the same a.b.c, so the special case would now only be a way for the four
 * to disagree.
 */
function syncAll(version, slot) {
  const code = versionCode(parseVersion(version), slot);
  const results = [
    syncTauri(version),
    syncGradle(version, code),
    syncReadme(version),
    syncWorkspaceDeps(version),
  ];
  for (const result of results) {
    if (result) console.log(`updated ${result.path} -> ${result.after}`);
  }
}

function slotFrom(args) {
  return args.includes('--develop') ? DEVELOP_SLOT : RELEASE_SLOT;
}

function positional(args) {
  return args.filter((arg) => !arg.startsWith('--'));
}

function main() {
  const [command, ...args] = process.argv.slice(2);

  switch (command) {
    case 'read':
      console.log(readWorkspaceVersion());
      break;

    case 'anchor': {
      // The range the counter is measured over, which is the thing ADR-008
      // records as having had no command before this one.
      const version = readWorkspaceVersion();
      const { anchor, commits } = counterState(version);
      const range = anchor ? `${anchor}..HEAD` : 'HEAD (the bump is not committed yet)';
      console.log(`${range}  ${commits} commit(s)  ${version} -> ${advanceVersion(version, commits)}`);
      break;
    }

    case 'advance': {
      const version = readWorkspaceVersion();
      console.log(advanceVersion(version, counterState(version).commits));
      break;
    }

    case 'bump': {
      const current = readWorkspaceVersion();
      const what = positional(args)[0];
      let version;
      if (!what) version = advanceVersion(current, counterState(current).commits);
      else if (what === 'minor' || what === 'major') {
        // Bump the advanced version, not the stale one in the tree, or the
        // commits since the last bump would be silently dropped on the floor.
        version = nextRelease(advanceVersion(current, counterState(current).commits), what);
      } else version = what;

      parseVersion(version);
      writeFileSync(CARGO_TOML, rewriteWorkspaceVersion(readFileSync(CARGO_TOML, 'utf8'), version));
      console.log(`updated ${CARGO_TOML} [workspace.package] -> ${version}`);
      syncAll(version, RELEASE_SLOT);
      break;
    }

    case 'next': {
      // What the commits say, applied to the version the counter has reached.
      //
      // The range is the commits since the last BUMP, not since the last
      // release. Since the last release is what made this a prompt rather than
      // an answer: with no release tag it spans the whole project, so it
      // re-counts commits an earlier bump already accounted for, and it reads
      // the two harmless `!` markers deep in that history as a major every
      // time. Anchoring it where the counter is anchored fixes both. See
      // ADR-008.
      //
      // 'patch' comes back unchanged, because c has already moved and there is
      // no third number left to bump. What this answers is the one decision
      // left: is this a minor, is it a major.
      const current = readWorkspaceVersion();
      const { anchor, commits } = counterState(current);
      const advanced = advanceVersion(current, commits);
      const range = anchor ? `${anchor}..HEAD` : null;
      const { subjects, bodies } = range ? commitsIn(range) : { subjects: [], bodies: [] };
      console.log(nextRelease(advanced, bumpFromCommits(subjects, bodies)));
      break;
    }

    case 'range':
      console.log(releaseRange());
      break;

    case 'code': {
      const version = positional(args)[0] ?? readWorkspaceVersion();
      console.log(versionCode(parseVersion(version), slotFrom(args)));
      break;
    }

    case 'sync': {
      const version = positional(args)[0] ?? readWorkspaceVersion();
      syncAll(version, slotFrom(args));
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
      if (existsSync(README)) {
        const text = readFileSync(README, 'utf8');
        const badge = /!\[Version\]\(https:\/\/img\.shields\.io\/badge\/version-([^-]+)-/.exec(text);
        if (badge && badge[1] !== version) {
          problems.push(`${README} badge says ${badge[1]}, Cargo.toml says ${version}`);
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
