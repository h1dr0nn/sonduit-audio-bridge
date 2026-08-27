#!/usr/bin/env node
/**
 * Tests for the pure parts of tools/version.mjs.
 *
 * Run with: node --test tools/
 *
 * These exist because two of the three bugs found while writing the release
 * tooling were in this file, and both were the kind that produce a plausible
 * wrong answer rather than a crash.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  MAX_DEV,
  RELEASE_SLOT,
  applyBump,
  bumpFromCommits,
  devCounter,
  parseVersion,
  splitCommitRecords,
  versionCode,
} from './version.mjs';

const RECORD = '\u001e';
const UNIT = '\u001f';

test('parseVersion rejects anything that is not a plain release version', () => {
  assert.deepEqual(parseVersion('1.2.3'), { major: 1, minor: 2, patch: 3 });
  for (const bad of ['1.2', 'v1.2.3', '1.2.3-dev.1', '', 'x']) {
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

test('applyBump resets the lower components', () => {
  assert.equal(applyBump('1.2.3', 'patch'), '1.2.4');
  assert.equal(applyBump('1.2.3', 'minor'), '1.3.0');
  assert.equal(applyBump('1.2.3', 'major'), '2.0.0');
});

test('versionCode is monotonic across dev builds and their release', () => {
  const sequence = [
    ['1.2.3', RELEASE_SLOT],
    ['1.3.0', 0],
    ['1.3.0', 7],
    ['1.3.0', 998],
    ['1.3.0', RELEASE_SLOT],
    ['1.3.1', 0],
    ['1.3.1', RELEASE_SLOT],
    ['2.0.0', 0],
    ['2.0.0', RELEASE_SLOT],
  ];

  const codes = sequence.map(([version, dev]) => versionCode(parseVersion(version), dev));
  for (let i = 1; i < codes.length; i += 1) {
    assert.ok(
      codes[i] > codes[i - 1],
      `${sequence[i].join(' dev.')} (${codes[i]}) must exceed ` +
        `${sequence[i - 1].join(' dev.')} (${codes[i - 1]})`,
    );
  }
});

test('a release outranks every dev build of the same version', () => {
  const release = versionCode(parseVersion('1.3.0'));
  for (let dev = 0; dev < RELEASE_SLOT; dev += 1) {
    assert.ok(versionCode(parseVersion('1.3.0'), dev) < release);
  }
});

test('versionCode stays inside the Play Store ceiling', () => {
  // The largest code the layout can produce must still fit under the cap.
  assert.equal(versionCode(parseVersion('209.99.99'), RELEASE_SLOT), 2_099_999_999);
  assert.ok(versionCode(parseVersion('209.99.99'), RELEASE_SLOT) <= 2_100_000_000);

  assert.throws(() => versionCode(parseVersion('210.0.0')));
  assert.throws(() => versionCode(parseVersion('1.100.0')));
  // patch is capped at 99, not 999: patch*1_000 must stay inside the minor field.
  assert.throws(() => versionCode(parseVersion('1.0.100')));
});
test('commit records survive the newline git puts between them', () => {
  // This is the exact shape `git log --format=%s<UNIT>%b<RECORD>` produces.
  const output = [
    `feat(core): first${UNIT}body one${RECORD}`,
    `\nfix(core): second${UNIT}${RECORD}`,
    `\nfeat(core): third${UNIT}BREAKING CHANGE: yes${RECORD}`,
    '\n',
  ].join('');

  const { subjects, bodies } = splitCommitRecords(output);

  assert.deepEqual(subjects, [
    'feat(core): first',
    'fix(core): second',
    'feat(core): third',
  ]);
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

test('the dev counter refuses to reach the reserved release slot', () => {
  // A CI run number would have hit 999 eventually, producing exactly the
  // release code for a build that is not the release.
  assert.equal(devCounter(0), 0);
  assert.equal(devCounter(MAX_DEV), MAX_DEV);
  assert.throws(() => devCounter(RELEASE_SLOT), /exceeds/);
  assert.throws(() => devCounter(5000), /exceeds/);
});

test('no dev build can ever produce the release code', () => {
  const release = versionCode(parseVersion('1.3.0'));
  for (let dev = 0; dev <= MAX_DEV; dev += 1) {
    assert.notEqual(versionCode(parseVersion('1.3.0'), dev), release);
  }
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
    // Dev builds of a version come before its release.
    for (const dev of [0, 1, 500, MAX_DEV, RELEASE_SLOT]) {
      const code = versionCode(version, dev);
      const label = version.major + '.' + version.minor + '.' + version.patch + ' dev.' + dev;
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
        for (const dev of [0, 99, MAX_DEV, RELEASE_SLOT]) {
          const code = versionCode({ major, minor, patch }, dev);
          const label = major + '.' + minor + '.' + patch + '+' + dev;
          assert.ok(!seen.has(code), label + ' collides with ' + seen.get(code) + ' at ' + code);
          seen.set(code, label);
        }
      }
    }
  }
});

test('each field is strictly narrower than the one above it', () => {
  // The invariant that makes the whole layout work.
  assert.ok(RELEASE_SLOT < 1_000, 'dev must fit under the patch multiplier');
  assert.throws(() => versionCode({ major: 1, minor: 0, patch: 100 }), /patch/);
  assert.throws(() => versionCode({ major: 1, minor: 100, patch: 0 }), /minor/);

  // The specific overflow that shipped and was caught in review.
  assert.ok(
    versionCode({ major: 1, minor: 0, patch: 99 }) <
      versionCode({ major: 1, minor: 1, patch: 0 }, 0),
    '1.0.99 must sort below 1.1.0-dev.0',
  );
});
