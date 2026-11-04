#!/usr/bin/env node
// Check or update application versions without rebuilding or changing dependencies.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const semver =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

// Locate actual object fields, including the root package in package-lock.json.
// JSON.parse validates syntax; token offsets let edits preserve whitespace and newlines.
function jsonFields(text, paths) {
  JSON.parse(text);
  const tokens = [
    ...text.matchAll(
      /"(?:\\.|[^"\\])*"|true|false|null|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}\[\]:,]/g,
    ),
  ];
  const found = new Map();
  let cursor = 0;
  function visit(path) {
    const first = tokens[cursor++];
    if (first[0] === '{') {
      while (tokens[cursor][0] !== '}') {
        const key = JSON.parse(tokens[cursor++][0]);
        cursor++; // colon
        visit([...path, key]);
        if (tokens[cursor][0] === ',') cursor++;
      }
      cursor++;
    } else if (first[0] === '[') {
      let index = 0;
      while (tokens[cursor][0] !== ']') {
        visit([...path, index++]);
        if (tokens[cursor][0] === ',') cursor++;
      }
      cursor++;
    }
    const key = JSON.stringify(path);
    if (paths.some((wanted) => JSON.stringify(wanted) === key)) {
      if (found.has(key)) throw new Error('Duplicate application version field.');
      const last = tokens[cursor - 1];
      found.set(key, {
        start: first.index,
        end: last.index + last[0].length,
        value: JSON.parse(text.slice(first.index, last.index + last[0].length)),
        json: true,
      });
    }
  }
  visit([]);
  return paths.map((path) => {
    const field = found.get(JSON.stringify(path));
    if (!field) throw new Error(`Missing JSON field ${JSON.stringify(path)}.`);
    return field;
  });
}

function cargoField(text, lock) {
  const blocks = [
    ...text.matchAll(/^\[\[?package\]?\][^\S\r\n]*\r?\n[\s\S]*?(?=^\[|(?![\s\S]))/gm),
  ];
  const matches = blocks.filter((block) =>
    lock
      ? block[0].startsWith('[[package]]') && /^name\s*=\s*"prism"\s*$/m.test(block[0])
      : block[0].startsWith('[package]'),
  );
  if (matches.length !== 1) throw new Error('Expected one Prism Cargo package.');
  const block = matches[0];
  const fields = [...block[0].matchAll(/^([ \t]*version[ \t]*=[ \t]*")([^"\r\n]+)(")/gm)];
  if (fields.length !== 1) throw new Error('Expected one Cargo package version.');
  const field = fields[0];
  const start = block.index + field.index + field[1].length;
  return [{ start, end: start + field[2].length, value: field[2], json: false }];
}

function main() {
  const args = process.argv.slice(2);
  let root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  let target;
  let hasRoot = false;
  for (let index = 0; index < args.length; index++) {
    if (args[index] === '--root') {
      if (hasRoot || !args[index + 1] || args[index + 1].startsWith('--')) {
        throw new Error('--root requires one directory.');
      }
      root = resolve(args[++index]);
      hasRoot = true;
    } else if (target === undefined) {
      target = args[index];
    } else {
      throw new Error('Expected --check, patch, minor, major or one explicit version.');
    }
  }
  target ??= '--check';
  const specs = [
    ['package.json', (text) => jsonFields(text, [['version']])],
    ['package-lock.json', (text) => jsonFields(text, [['version'], ['packages', '', 'version']])],
    ['src-tauri/tauri.conf.json', (text) => jsonFields(text, [['version']])],
    ['src-tauri/Cargo.toml', (text) => cargoField(text, false)],
    ['src-tauri/Cargo.lock', (text) => cargoField(text, true)],
  ];
  // Read and validate every file before the first write.
  const files = specs.map(([path, fields]) => {
    const fullPath = join(root, path);
    const text = readFileSync(fullPath, 'utf8');
    return { path: fullPath, text, fields: fields(text) };
  });
  const versions = files.flatMap((file) => file.fields.map((field) => field.value));
  const current = versions[0];
  if (new Set(versions).size !== 1 || typeof current !== 'string' || !semver.test(current)) {
    throw new Error(
      'Application versions differ or are invalid. Repair the five version files first.',
    );
  }
  if (target === '--check') {
    console.log(`Version ${current} is consistent across manifests and lockfiles.`);
    return;
  }
  let next = target;
  if (['patch', 'minor', 'major'].includes(target)) {
    const match = current.match(semver);
    const parts = match.slice(1, 4).map(BigInt);
    const part = { major: 0, minor: 1, patch: 2 }[target];
    // Like npm version: finishing a prerelease keeps its core version when possible.
    const finishPrerelease =
      match[4] && (part === 2 || parts.slice(part + 1).every((value) => value === 0n));
    if (!finishPrerelease) parts[part]++;
    for (let index = part + 1; index < parts.length; index++) parts[index] = 0n;
    next = parts.join('.');
  }
  if (!semver.test(next)) throw new Error(`Not a canonical semantic version: ${next}`);
  const replacements = files.map((file) => {
    let result = file.text;
    for (const field of [...file.fields].sort((a, b) => b.start - a.start)) {
      result =
        result.slice(0, field.start) +
        (field.json ? JSON.stringify(next) : next) +
        result.slice(field.end);
    }
    return { ...file, result };
  });
  const written = [];
  try {
    for (const file of replacements) {
      // Refuse a concurrent edit rather than overwrite it with the earlier snapshot.
      if (readFileSync(file.path, 'utf8') !== file.text)
        throw new Error('Version files changed during update.');
      written.push(file);
      writeFileSync(file.path, file.result);
    }
  } catch (cause) {
    let restored = true;
    for (const file of written.reverse()) {
      try {
        writeFileSync(file.path, file.text);
      } catch {
        restored = false;
      }
    }
    throw new Error(
      `${cause.message}${restored ? ' Original versions restored.' : ' Rollback incomplete; inspect version files.'}`,
    );
  }
  console.log(`Version ${current} -> ${next}; dependency versions unchanged.`);
}
try {
  main();
} catch (cause) {
  console.error(`Version command failed: ${cause.message}`);
  process.exitCode = 1;
}
