#!/usr/bin/env node
/**
 * Tests for the pure parts of tools/version.mjs.
 *
 * Run with: node --test tools/version.test.mjs
 *
 * These exist because two of the three bugs found while writing the release
 * tooling were in this file, and both were the kind that produce a plausible
 * wrong answer rather than a crash.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  DEVELOP_SLOT,
  MAX_COUNTER,
  RELEASE_SLOT,
  advanceVersion,
  bumpFromCommits,
  findAnchor,
  nextRelease,
  parseVersion,
  parseWorkspaceVersion,
  rewriteWorkspaceDeps,
  rewriteWorkspaceVersion,
  splitCommitRecords,
  versionCode,
} from './version.mjs';

// The ASCII record and unit separators, built rather than written, so the raw
// control bytes never end up in this file where an editor or a shell could
// mangle them.
const RECORD = String.fromCharCode(30);
const UNIT = String.fromCharCode(31);
const NL = String.fromCharCode(10);

test('parseVersion rejects anything that is not a plain a.b.c version', () => {
  assert.deepEqual(parseVersion('1.2.3'), { major: 1, minor: 2, patch: 3 });
  // The -dev.N form is gone, and a version carrying one must not parse: the
  // counter lives in c now, so such a string could only be a stale caller.
  for (const bad of ['1.2', 'v1.2.3', '1.2.3-dev.1', '1.2.75+9f316eda', '', 'x']) {
    assert.throws(() => parseVersion(bad), undefined, `should reject ${bad}`);
  }
});

test('bump is driven by the highest-ranking commit in the range', () => {
  assert.equal(bumpFromCommits(['fix(core): a']), 'patch');
  assert.equal(bumpFromCommits(['chore: a', 'docs: b']), 'patch');
  assert.equal(bumpFromCommits(['fix(core): a', 'feat(core): b']), 'minor');
  assert.equal(bumpFromCommits(['feat(core)!: b']), 'major');
  assert.equal(bumpFromCommits(['fix(core): a'], ['BREAKING CHANGE: gone']), 'major');
  // A breaking change anywhere wins, even behind later ordinary commits.
  assert.equal(bumpFromCommits(['feat(core)!: a', 'fix(core): b']), 'major');
});

test('unparseable subjects do not contribute a bump', () => {
  assert.equal(bumpFromCommits(['Merge branch develop', 'not a commit message']), 'patch');
});

test('a release bump moves a or b, and patch leaves the version alone', () => {
  // c is the build counter, not a patch number. It has already moved with
  // every commit, so a release of fixes is a tag on the version the counter
  // produced -- there is no third number left to bump.
  assert.equal(nextRelease('1.2.75', 'patch'), '1.2.75');
  assert.equal(nextRelease('1.2.75', 'minor'), '1.3.0');
  assert.equal(nextRelease('1.2.75', 'major'), '2.0.0');
});

//
// The build counter
//

test('the counter advances by one per commit', () => {
  assert.equal(advanceVersion('1.2.0', 0), '1.2.0');
  assert.equal(advanceVersion('1.2.0', 1), '1.2.1');
  assert.equal(advanceVersion('1.2.75', 1), '1.2.76');
  assert.equal(advanceVersion('1.2.75', 24), '1.2.99');
});

test('c rolls into b at 100 rather than overflowing the versionCode field', () => {
  // The ceiling is not a style choice: patch*1_000 has to stay under the
  // 100_000 minor multiplier, so 1.2.100 has no representable code.
  assert.equal(advanceVersion('1.2.99', 1), '1.3.0');
  assert.equal(advanceVersion('1.2.0', 100), '1.3.0');
  assert.equal(advanceVersion('1.2.98', 2), '1.3.0');
});

test('the roll is a carry, not a reset, so the counter keeps its rate', () => {
  // 1.2.99 -> 1.3.0 -> 1.3.1 across three consecutive commits, whether or not
  // anybody bumped the tree in between.
  assert.equal(advanceVersion('1.2.99', 0), '1.2.99');
  assert.equal(advanceVersion('1.2.99', 1), '1.3.0');
  assert.equal(advanceVersion('1.2.99', 2), '1.3.1');
  // Several rolls at once, if nobody has bumped for a long time.
  assert.equal(advanceVersion('1.2.0', 250), '1.4.50');
  assert.equal(advanceVersion('1.0.50', 100), '1.1.50');
});

test('a roll past the minor field fails loudly instead of wrapping', () => {
  // minor*100_000 must stay under the 10_000_000 major multiplier. Wrapping
  // here would produce a code below one already published, which Play Store
  // never forgets.
  assert.throws(() => advanceVersion('1.99.0', 100), /minor/);
  assert.throws(() => advanceVersion('1.99.99', 1), /minor/);
  assert.equal(advanceVersion('1.98.99', 1), '1.99.0', 'the last roll that fits is allowed');
});

test('advanceVersion refuses a count that is not a count', () => {
  for (const bad of [-1, 1.5, NaN, '3', null, undefined]) {
    assert.throws(() => advanceVersion('1.2.0', bad), undefined, `should reject ${bad}`);
  }
});

test('versionCode is monotonic across the rollover', () => {
  // The rollover must not be the one place the code goes backwards. It is the
  // exact boundary the old scheme never had to cross, because c never moved.
  const before = versionCode(parseVersion(advanceVersion('1.2.99', 0)));
  const after = versionCode(parseVersion(advanceVersion('1.2.99', 1)));
  assert.equal(before, 10_299_999);
  assert.equal(after, 10_300_999);
  assert.ok(after > before, 'the roll from 1.2.99 to 1.3.0 must raise the code');

  // And every step of a hundred commits either side of it, in both slots.
  let previous = -1;
  for (let commits = 0; commits <= 200; commits += 1) {
    for (const slot of [DEVELOP_SLOT, RELEASE_SLOT]) {
      const code = versionCode(parseVersion(advanceVersion('1.2.0', commits)), slot);
      assert.ok(code > previous, `commit ${commits} slot ${slot} (${code}) must exceed ${previous}`);
      previous = code;
    }
  }
});

test('the counter anchors to the commit that wrote the version, not to a release', () => {
  // Newest first, as `git log -- Cargo.toml` gives them. The anchor is the
  // commit that introduced the version now in the tree.
  const history = [
    { sha: 'bbbb', version: '1.2.76' },
    { sha: 'aaaa', version: '1.2.0' },
    { sha: '9999', version: '1.1.0' },
  ];
  assert.equal(findAnchor(history, '1.2.76'), 'bbbb');

  // A commit that touched Cargo.toml without changing the version -- adding a
  // dependency, say -- is part of the same run, and the anchor is the oldest
  // commit of it, which is the one that actually wrote the number.
  const withNoise = [
    { sha: 'dddd', version: '1.2.76' },
    { sha: 'cccc', version: '1.2.76' },
    { sha: 'aaaa', version: '1.2.0' },
  ];
  assert.equal(findAnchor(withNoise, '1.2.76'), 'cccc');
});

test('a bump sitting in the working tree anchors to HEAD', () => {
  // Nothing committed carries the new number yet, so nothing has happened
  // since it was written and the counter must not move. This is what makes
  // `bump` idempotent: running it twice in a row writes the same version.
  const history = [
    { sha: 'aaaa', version: '1.2.0' },
    { sha: '9999', version: '1.1.0' },
  ];
  assert.equal(findAnchor(history, '1.2.76'), null);
  assert.equal(findAnchor([], '1.2.76'), null);
});

test('an older commit carrying the same version is not the anchor', () => {
  // The version was lowered back to 1.1.0 once in this project's history. The
  // anchor is where the CURRENT run of it began, not where the number was
  // first used, or the counter would leap by every commit in between.
  const history = [
    { sha: 'eeee', version: '1.1.0' },
    { sha: 'dddd', version: '2.0.0' },
    { sha: 'cccc', version: '1.1.0' },
  ];
  assert.equal(findAnchor(history, '1.1.0'), 'eeee');
});

test('unreadable history ends the run instead of anchoring to it', () => {
  // cargoVersionHistory records null for a Cargo.toml with no workspace table,
  // which is what the Harmonix end of this history looks like.
  const history = [
    { sha: 'aaaa', version: '1.2.76' },
    { sha: 'old1', version: null },
  ];
  assert.equal(findAnchor(history, '1.2.76'), 'aaaa');
  assert.equal(findAnchor([{ sha: 'old1', version: null }], '1.2.76'), null);
});

test('the counter resets after a bump and the version keeps climbing', () => {
  // The whole point of anchoring to the bump: a rolled version must not be
  // handed the same overflowing count again on the next commit. Walk it.
  //
  //   1.2.99 + 1 commit -> 1.3.0, which is written into the tree by `bump`
  //   that bump commit becomes the anchor, so the count restarts at 0
  //   the next commit is 1.3.1, NOT another roll to 1.4.0
  const rolled = advanceVersion('1.2.99', 1);
  assert.equal(rolled, '1.3.0');

  const afterBump = [{ sha: 'roll', version: rolled }];
  assert.equal(findAnchor(afterBump, rolled), 'roll', 'the bump commit is the new anchor');

  assert.equal(advanceVersion(rolled, 0), '1.3.0', 'at the bump commit itself');
  assert.equal(advanceVersion(rolled, 1), '1.3.1', 'one commit later');
  assert.equal(advanceVersion(rolled, 99), '1.3.99');

  // Anchored to the last release instead -- which is what the old dev counter
  // did, and there has never been a release tag -- the count would still be
  // 100 here and the version would roll a second time off the same commits.
  assert.equal(advanceVersion(rolled, 100), '1.4.0', 'the failure the anchor prevents');
});

test('a hand bump of b resets the counter too', () => {
  // The maintainer deciding "this is a minor" is the same event as a roll: it
  // writes a.b.c into the tree, and that commit is what the next hundred are
  // counted from.
  const decided = nextRelease(advanceVersion('1.2.40', 5), 'minor');
  assert.equal(decided, '1.3.0');
  assert.equal(advanceVersion(decided, 1), '1.3.1');
  assert.ok(
    versionCode(parseVersion(decided)) > versionCode(parseVersion(advanceVersion('1.2.40', 5))),
    'a decided minor must outrank the counter version it was decided from',
  );
});

//
// The versionCode layout
//

test('a release outranks the develop build of the same version', () => {
  // The version string no longer says which kind of build it is, so this is
  // the only place the distinction survives in the code.
  assert.equal(versionCode(parseVersion('1.3.0'), DEVELOP_SLOT), 10_300_000);
  assert.equal(versionCode(parseVersion('1.3.0'), RELEASE_SLOT), 10_300_999);
  assert.ok(
    versionCode(parseVersion('1.3.0'), DEVELOP_SLOT) <
      versionCode(parseVersion('1.3.0'), RELEASE_SLOT),
  );
  assert.equal(versionCode(parseVersion('1.3.0')), 10_300_999, 'the release slot is the default');
});

test('versionCode is monotonic across develop builds, releases and bumps', () => {
  const sequence = [
    ['1.2.3', RELEASE_SLOT],
    ['1.3.0', DEVELOP_SLOT],
    ['1.3.0', RELEASE_SLOT],
    ['1.3.1', DEVELOP_SLOT],
    ['1.3.1', RELEASE_SLOT],
    ['1.3.99', DEVELOP_SLOT],
    ['1.4.0', DEVELOP_SLOT],
    ['2.0.0', DEVELOP_SLOT],
    ['2.0.0', RELEASE_SLOT],
  ];

  const codes = sequence.map(([version, slot]) => versionCode(parseVersion(version), slot));
  for (let i = 1; i < codes.length; i += 1) {
    assert.ok(
      codes[i] > codes[i - 1],
      `${sequence[i].join(' slot ')} (${codes[i]}) must exceed ` +
        `${sequence[i - 1].join(' slot ')} (${codes[i - 1]})`,
    );
  }
});

test('versionCode stays inside the Play Store ceiling', () => {
  // The largest code the layout can produce must still fit under the cap.
  assert.equal(versionCode(parseVersion('209.99.99'), RELEASE_SLOT), 2_099_999_999);
  assert.ok(versionCode(parseVersion('209.99.99'), RELEASE_SLOT) <= 2_100_000_000);

  assert.throws(() => versionCode(parseVersion('210.0.0')));
  assert.throws(() => versionCode(parseVersion('1.100.0')));
  // The counter is capped at 99, not 999: patch*1_000 must stay inside the
  // minor field. This is the cap the rollover exists to respect.
  assert.throws(() => versionCode(parseVersion('1.0.100')));
  assert.equal(MAX_COUNTER, 99);
});

test('versionCode is monotonic across the WHOLE version space, not just the examples', () => {
  // The original suite only walked the six versions printed in ADR-008, which
  // is exactly how a patch overflow into the minor field survived review.
  // Walk the representable space in semver order instead.
  const versions = [];
  for (const major of [0, 1, 2]) {
    for (const minor of [0, 1, 50, 98, 99]) {
      for (const patch of [0, 1, 50, 98, 99]) {
        versions.push({ major, minor, patch });
      }
    }
  }

  let previous = -1;
  let previousLabel = 'start';
  for (const version of versions) {
    // A develop build of a version comes before its release.
    for (const slot of [DEVELOP_SLOT, RELEASE_SLOT]) {
      const code = versionCode(version, slot);
      const label = version.major + '.' + version.minor + '.' + version.patch + ' slot ' + slot;
      assert.ok(
        code > previous,
        label + ' (' + code + ') must exceed ' + previousLabel + ' (' + previous + ')',
      );
      previous = code;
      previousLabel = label;
    }
  }
});

test('no two distinct versions share a code', () => {
  const seen = new Map();
  for (const major of [0, 1]) {
    for (const minor of [0, 9, 99]) {
      for (const patch of [0, 9, 99]) {
        for (const slot of [DEVELOP_SLOT, RELEASE_SLOT]) {
          const code = versionCode({ major, minor, patch }, slot);
          const label = major + '.' + minor + '.' + patch + ' slot ' + slot;
          assert.ok(!seen.has(code), label + ' collides with ' + seen.get(code) + ' at ' + code);
          seen.set(code, label);
        }
      }
    }
  }
});

test('each field is strictly narrower than the one above it', () => {
  // The invariant that makes the whole layout work.
  assert.ok(RELEASE_SLOT < 1_000, 'the slot must fit under the patch multiplier');
  assert.ok(DEVELOP_SLOT >= 0 && DEVELOP_SLOT < RELEASE_SLOT);
  assert.throws(() => versionCode({ major: 1, minor: 0, patch: 100 }), /patch/);
  assert.throws(() => versionCode({ major: 1, minor: 100, patch: 0 }), /minor/);
  assert.throws(() => versionCode({ major: 1, minor: 0, patch: 0 }, 1_000), /slot/);

  // The specific overflow that shipped and was caught in review.
  assert.ok(
    versionCode({ major: 1, minor: 0, patch: 99 }) <
      versionCode({ major: 1, minor: 1, patch: 0 }, DEVELOP_SLOT),
    '1.0.99 must sort below a develop build of 1.1.0',
  );
});

test('the codes already published stay below every code the scheme can produce', () => {
  // 10_200_999 is on a phone right now, from the 1.2.0 tree, and 10_200_075
  // was the last develop build of it under the -dev.N scheme. Moving the
  // counter into c must not produce anything below either, or the build will
  // not install and the code can never be reused.
  const installed = 10_200_999;
  assert.ok(versionCode(parseVersion('1.2.76'), DEVELOP_SLOT) > installed);
  assert.ok(versionCode(parseVersion('1.2.76'), RELEASE_SLOT) > installed);
  assert.ok(versionCode(parseVersion('1.2.1'), DEVELOP_SLOT) > installed);
});

//
// Reading and writing Cargo.toml
//

test('commit records survive the newline git puts between them', () => {
  // This is the exact shape `git log --format=%s<UNIT>%b<RECORD>` produces.
  const output = [
    `feat(core): first${UNIT}body one${RECORD}`,
    `\nfix(core): second${UNIT}${RECORD}`,
    `\nfeat(core): third${UNIT}BREAKING CHANGE: yes${RECORD}`,
    '\n',
  ].join('');

  const { subjects, bodies } = splitCommitRecords(output);

  assert.deepEqual(subjects, ['feat(core): first', 'fix(core): second', 'feat(core): third']);
  assert.equal(bodies.length, subjects.length, 'lists must stay aligned');
  assert.equal(bodies[1], '', 'an empty body must not collapse the list');
  assert.equal(bumpFromCommits(subjects, bodies), 'major');
});

test('an empty body never shifts a later body onto the wrong subject', () => {
  const output = [
    `fix(core): a${UNIT}${RECORD}`,
    `\nfix(core): b${UNIT}BREAKING CHANGE: on b${RECORD}`,
  ].join('');

  const { subjects, bodies } = splitCommitRecords(output);
  assert.equal(subjects[0], 'fix(core): a');
  assert.equal(bodies[0], '');
  assert.equal(subjects[1], 'fix(core): b');
  assert.match(bodies[1], /BREAKING CHANGE/);
});

const CARGO = [
  '[workspace.package]',
  'version = "1.2.0"',
  'edition = "2021"',
  '',
  '[workspace.dependencies]',
  'sonduit-core = { path = "crates/sonduit-core", version = "1.2.0" }',
  'sonduit-playback-android = { path = "crates/sonduit-playback-android", version = "1.2.0" }',
  '',
  'thiserror = "2"',
].join(NL);

test('the workspace version is read from its own table', () => {
  assert.equal(parseWorkspaceVersion(CARGO), '1.2.0');
  assert.throws(() => parseWorkspaceVersion('[package]' + NL + 'version = "1.0.0"'), /workspace/);
});

test('a bump rewrites the workspace version and nothing else', () => {
  // `version = "..."` appears on every path dependency too. Rewriting those
  // here as well would make this and rewriteWorkspaceDeps disagree about what
  // they had written, and the disagreement would only show up as a workspace
  // that no longer resolves.
  const after = rewriteWorkspaceVersion(CARGO, '1.2.76');

  assert.equal(parseWorkspaceVersion(after), '1.2.76');
  assert.match(after, /sonduit-core = .*version = "1\.2\.0" \}/, 'deps are not this function');
  assert.match(after, /edition = "2021"/);
  assert.match(after, /thiserror = "2"/);
  assert.equal(after.split(NL).length, CARGO.split(NL).length, 'no line was added or lost');
});

test('a bump refuses a version the scheme cannot represent', () => {
  for (const bad of ['1.2', '1.2.3-dev.4', 'v1.2.3', '']) {
    assert.throws(() => rewriteWorkspaceVersion(CARGO, bad), undefined, `should reject ${bad}`);
  }
});

test('workspace dependency requirements follow the workspace version', () => {
  // These drifted the first time the version changed and broke the whole
  // workspace: every crate required ^0.1.0 while every crate was 1.0.4.
  const after = rewriteWorkspaceDeps(CARGO, '1.2.76');

  assert.match(after, /sonduit-core = \{ path = "crates\/sonduit-core", version = "1\.2\.76" \}/);
  assert.match(after, /sonduit-playback-android = .*version = "1\.2\.76" \}/);
  assert.match(after, /thiserror = "2"/, 'an unrelated dependency was rewritten');
});

test('a third-party dependency that looks similar is left alone', () => {
  // The pattern must not touch anything outside this workspace, or a bump
  // would silently change a real external requirement.
  const before = 'sonduit-lookalike-external = "0.1.0"';
  assert.equal(rewriteWorkspaceDeps(before, '9.9.9'), before);
});
