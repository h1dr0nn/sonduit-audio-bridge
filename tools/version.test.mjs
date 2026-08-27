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
  RELEASE_SLOT,
  applyBump,
  bumpFromCommits,
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
  // The largest code the layout can produce must still fit.
  assert.equal(versionCode(parseVersion('208.99.999'), RELEASE_SLOT), 2_090_899_999);
  assert.ok(versionCode(parseVersion('208.99.999'), RELEASE_SLOT) <= 2_100_000_000);
  assert.throws(() => versionCode(parseVersion('209.0.0')));
  assert.throws(() => versionCode(parseVersion('1.100.0')));
  assert.throws(() => versionCode(parseVersion('1.0.1000')));
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
