import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

const command = resolve('scripts/bump-version.mjs');
const paths = [
  'package.json',
  'package-lock.json',
  'src-tauri/tauri.conf.json',
  'src-tauri/Cargo.toml',
  'src-tauri/Cargo.lock',
  'installer/src-tauri/tauri.conf.json',
  'installer/src-tauri/Cargo.toml',
  'installer/src-tauri/Cargo.lock',
];
function fixture(t, version = '0.1.0') {
  const root = mkdtempSync(join(tmpdir(), 'prism-version-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'src-tauri'));
  mkdirSync(join(root, 'installer/src-tauri'), { recursive: true });
  // Nested versions come before the root version to catch naive first-match replacement.
  const content = [
    `{ "example": { "version": "0.1.0" }, "name": "prism", "version": "${version}", "dependencies": { "example": "0.1.0" } }\r\n`,
    JSON.stringify(
      {
        name: 'prism',
        version,
        lockfileVersion: 3,
        packages: {
          '': { name: 'prism', version },
          'node_modules/example': { version: '0.1.0', integrity: 'preserved' },
        },
      },
      null,
      2,
    ) + '\n',
    `{ "version": "${version}", "bundle": { "example": "keep" } }\n`,
    `[package]\r\nname = "prism"\r\nauthors = ["Prism"]\r\nversion = "${version}" # preserve comment\r\n[dependencies]\r\nexample = "0.1.0"\r\n`,
    `# generated lockfile\nversion = 4\n\n[[package]]\nname = "example"\nversion = "0.1.0"\n\n[[package]]\nname = "prism"\nversion = "${version}"\ndependencies = [\n "example",\n]\n`,
  ];
  content.push(
    content[2],
    content[3].replace('name = "prism"', 'name = "prism-setup"'),
    content[4].replace('name = "prism"', 'name = "prism-setup"'),
  );
  paths.forEach((path, i) => writeFileSync(join(root, path), content[i]));
  const snapshot = () => paths.map((path) => readFileSync(join(root, path), 'utf8'));
  const run = (...args) =>
    spawnSync(process.execPath, [command, '--root', root, ...args], {
      encoding: 'utf8',
      windowsHide: true,
    });
  return { root, content, snapshot, run };
}

test('check validates all version fields without writing', (t) => {
  const f = fixture(t);
  assert.equal(f.run('--check').status, 0);
  assert.deepEqual(f.snapshot(), f.content);
});

test('bump updates manifests and lockfiles while preserving dependencies and formatting', (t) => {
  const f = fixture(t);
  const result = f.run('minor');
  assert.equal(result.status, 0, result.stderr);
  const after = f.snapshot();
  const expected = f.content.map((text, i) => {
    if (i === 0)
      return text.replace(
        '"name": "prism", "version": "0.1.0"',
        '"name": "prism", "version": "0.2.0"',
      );
    if (i === 1)
      return text
        .replaceAll('"version": "0.1.0"', '"version": "0.2.0"')
        .replace(
          '"node_modules/example": {\n      "version": "0.2.0"',
          '"node_modules/example": {\n      "version": "0.1.0"',
        );
    if (i === 4 || i === 7) {
      const name = i === 4 ? 'prism' : 'prism-setup';
      return text.replace(
        `name = "${name}"\nversion = "0.1.0"`,
        `name = "${name}"\nversion = "0.2.0"`,
      );
    }
    return text
      .replace('version = "0.1.0"', 'version = "0.2.0"')
      .replace('"version": "0.1.0"', '"version": "0.2.0"');
  });
  assert.deepEqual(after, expected);
  assert.equal(f.run('--check').status, 0);
});

test('explicit prerelease and build metadata remain consistent and patch finishes prerelease', (t) => {
  const f = fixture(t);
  let result = f.run('1.2.3-rc.1+local.5');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(f.run('--check').status, 0);
  result = f.run('patch');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(f.snapshot()[0]).version, '1.2.3');
  assert.equal(f.run('major').status, 0);
  assert.equal(JSON.parse(f.snapshot()[0]).version, '2.0.0');
});

test('noncanonical versions and malformed arguments leave every file unchanged', (t) => {
  const f = fixture(t);
  for (const args of [
    ['01.2.3'],
    ['1.2'],
    ['1.2.3-01'],
    ['1.2.3-rc..1'],
    ['1.2.3+'],
    ['patch', 'extra'],
    ['--root'],
  ]) {
    const result = f.run(...args);
    assert.notEqual(result.status, 0, args.join(' '));
    assert.deepEqual(f.snapshot(), f.content);
  }
});

test('a stale lockfile is rejected before any version is written', (t) => {
  const f = fixture(t);
  const path = join(f.root, paths[1]);
  writeFileSync(path, f.content[1].replace('"version": "0.1.0"', '"version": "0.0.9"'));
  const before = f.snapshot();
  assert.notEqual(f.run('--check').status, 0);
  assert.notEqual(f.run('patch').status, 0);
  assert.deepEqual(f.snapshot(), before);
});

test('missing or malformed later files cannot leave partially bumped manifests', (t) => {
  const f = fixture(t);
  writeFileSync(join(f.root, paths[4]), 'invalid Cargo lockfile');
  const before = f.snapshot();
  assert.notEqual(f.run('patch').status, 0);
  assert.deepEqual(f.snapshot(), before);
});

test('stale setup metadata and lockfile fail before changing any app version', (t) => {
  for (const index of [5, 6, 7]) {
    const f = fixture(t);
    writeFileSync(join(f.root, paths[index]), f.content[index].replaceAll('0.1.0', '0.0.9'));
    const before = f.snapshot();
    assert.notEqual(f.run('--check').status, 0);
    assert.notEqual(f.run('patch').status, 0);
    assert.deepEqual(f.snapshot(), before);
  }
});

test('a different Cargo package cannot masquerade as setup metadata', (t) => {
  const f = fixture(t);
  writeFileSync(join(f.root, paths[6]), f.content[6].replace('prism-setup', 'different-installer'));
  const before = f.snapshot();
  assert.notEqual(f.run('patch').status, 0);
  assert.deepEqual(f.snapshot(), before);
});
