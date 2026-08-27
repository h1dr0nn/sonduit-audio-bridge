#!/usr/bin/env node
/**
 * Fails when a .rs or .kt file contains non-ASCII outside the body of a plain
 * `//` comment.
 *
 * The project rule, from the brief:
 *   - identifiers, type and function names, log and error strings: English
 *   - rustdoc `///` and `//!`, and KDoc block docs: English, because they
 *     become public API documentation
 *   - plain `//` inline comments: Vietnamese is allowed, for explaining hard
 *     domain logic such as jitter maths or protocol quirks
 *
 * So `//` comment bodies are the only place non-ASCII may appear. Everything
 * else, including string literals, must be ASCII.
 *
 * Usage:
 *   node tools/lint/check-source-ascii.mjs [paths...]
 * Defaults to scanning crates/, desktop/src-tauri/src/ and android/.
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, extname, relative, sep } from 'node:path';

const DEFAULT_ROOTS = ['crates', 'desktop/src-tauri/src', 'android'];
const EXTENSIONS = new Set(['.rs', '.kt', '.kts']);
const SKIP_DIRS = new Set(['target', 'node_modules', '.git', 'build', '.gradle', 'gen']);

/**
 * Blank out every region of a line that is NOT the body of a plain `//`
 * comment, replacing it with spaces so column numbers survive.
 *
 * Returns { masked, blockDepth } where `masked` keeps only characters that are
 * allowed to be non-ASCII... inverted: `masked` keeps the characters that must
 * be ASCII, and blanks the ones that may be anything.
 */
function maskAllowedRegions(line, state) {
  const out = Array.from(line);
  let i = 0;

  const blank = (from, to) => {
    for (let k = from; k < to && k < out.length; k += 1) out[k] = ' ';
  };

  while (i < line.length) {
    // Inside a block comment: /* */ is NOT an allowed non-ASCII region, so it
    // stays in the masked output and will be flagged.
    if (state.inBlockComment) {
      const end = line.indexOf('*/', i);
      if (end === -1) return { masked: out.join(''), state };
      i = end + 2;
      state.inBlockComment = false;
      continue;
    }

    // Inside a multi-line raw string.
    if (state.rawStringHashes !== null) {
      const terminator = `"${'#'.repeat(state.rawStringHashes)}`;
      const end = line.indexOf(terminator, i);
      if (end === -1) return { masked: out.join(''), state };
      i = end + terminator.length;
      state.rawStringHashes = null;
      continue;
    }

    const two = line.slice(i, i + 2);

    if (two === '/*') {
      state.inBlockComment = true;
      i += 2;
      continue;
    }

    if (two === '//') {
      const third = line[i + 2];
      // `///` and `//!` are documentation and must stay ASCII, so they are
      // left in the masked output. A plain `//` body is blanked out.
      if (third === '/' || third === '!') {
        return { masked: out.join(''), state };
      }
      blank(i + 2, line.length);
      return { masked: out.join(''), state };
    }

    // Raw string: r"..." or r#"..."#
    const rawMatch = /^r(#*)"/.exec(line.slice(i));
    if (rawMatch && (i === 0 || !/[A-Za-z0-9_]/.test(line[i - 1]))) {
      const hashes = rawMatch[1].length;
      const start = i + rawMatch[0].length;
      const terminator = `"${'#'.repeat(hashes)}`;
      const end = line.indexOf(terminator, start);
      if (end === -1) {
        state.rawStringHashes = hashes;
        return { masked: out.join(''), state };
      }
      i = end + terminator.length;
      continue;
    }

    if (line[i] === '"') {
      i += 1;
      while (i < line.length) {
        if (line[i] === '\\') {
          i += 2;
          continue;
        }
        if (line[i] === '"') {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }

    // A char literal, but not a Rust lifetime such as `'a`.
    if (line[i] === "'") {
      const charLiteral = /^'(\\.|[^\\'])'/.exec(line.slice(i));
      if (charLiteral) {
        i += charLiteral[0].length;
        continue;
      }
    }

    i += 1;
  }

  return { masked: out.join(''), state };
}

function* walk(root) {
  let entries;
  try {
    entries = readdirSync(root);
  } catch {
    return;
  }
  for (const entry of entries) {
    if (SKIP_DIRS.has(entry)) continue;
    const path = join(root, entry);
    const info = statSync(path);
    if (info.isDirectory()) {
      yield* walk(path);
    } else if (EXTENSIONS.has(extname(entry))) {
      yield path;
    }
  }
}

function checkFile(path) {
  const problems = [];
  const text = readFileSync(path, 'utf8');
  const state = { inBlockComment: false, rawStringHashes: null };

  text.split(/\r?\n/).forEach((line, index) => {
    const { masked } = maskAllowedRegions(line, state);
    for (let column = 0; column < masked.length; column += 1) {
      const code = masked.codePointAt(column);
      if (code > 0x7f) {
        problems.push({
          path,
          line: index + 1,
          column: column + 1,
          char: masked[column],
          code,
          text: line.trim(),
        });
        break; // one report per line is enough to find it
      }
    }
  });

  return problems;
}

const roots = process.argv.slice(2);
const targets = roots.length > 0 ? roots : DEFAULT_ROOTS;

const problems = [];
let scanned = 0;
for (const root of targets) {
  for (const path of walk(root)) {
    scanned += 1;
    problems.push(...checkFile(path));
  }
}

if (problems.length > 0) {
  console.error('Non-ASCII found outside a plain // comment body:\n');
  for (const problem of problems) {
    const where = `${relative('.', problem.path).split(sep).join('/')}:${problem.line}:${problem.column}`;
    const code = `U+${problem.code.toString(16).toUpperCase().padStart(4, '0')}`;
    console.error(`  ${where}  ${JSON.stringify(problem.char)} (${code})`);
    console.error(`    ${problem.text}`);
  }
  console.error(
    `\n${problems.length} problem(s) in ${scanned} file(s).\n` +
      'English is required for identifiers, log and error strings, and for\n' +
      '/// and //! documentation. Vietnamese belongs in plain // comments.',
  );
  process.exit(1);
}

console.log(`OK: ${scanned} file(s) scanned, no non-ASCII outside // comment bodies.`);
