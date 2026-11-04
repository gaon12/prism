import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { buildManifest, updateFiles } from '../../scripts/make-latest-json.mjs';

const base = {
  version: '0.2.0',
  repo: 'gaon12/prism',
  notes: '한글과 🙂 release notes',
  pubDate: '2026-11-05T00:00:00.000Z',
  readSignature: () => 'synthetic pure-model signature',
  verifyArtifact: () => {},
};
const fixtures = resolve('tests/fixtures/updates');
const verifier =
  process.env.PRISM_UPDATE_VERIFIER ??
  resolve(
    'tools/update-verifier/target/debug',
    process.platform === 'win32' ? 'prism-update-verifier.exe' : 'prism-update-verifier',
  );
const key = readFileSync(join(fixtures, 'public.pub'), 'utf8');
function fixture(t, version = base.version) {
  const directory = mkdtempSync(join(tmpdir(), 'prism-release-manifest-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const files = [...new Set(Object.values(updateFiles(version)))];
  for (const file of files) {
    copyFileSync(join(fixtures, 'payload.bin'), join(directory, file));
    copyFileSync(join(fixtures, 'payload.bin.sig'), join(directory, `${file}.sig`));
  }
  const run = (extra = [], environment = {}) =>
    spawnSync(
      process.execPath,
      [
        resolve('scripts/make-latest-json.mjs'),
        '--dir',
        directory,
        '--version',
        version,
        '--verifier',
        verifier,
        ...extra,
      ],
      {
        env: { ...process.env, PRISM_UPDATER_PUBLIC_KEY: key, ...environment },
        encoding: 'utf8',
        windowsHide: true,
      },
    );
  return { directory, files, run, output: join(directory, 'latest.json') };
}

test('all updater platforms require verification, with one verification per universal artifact', () => {
  const checked = [];
  const manifest = buildManifest({
    ...base,
    verifyArtifact: (file, version) => checked.push([file, version]),
  });
  assert.equal(Object.keys(manifest.platforms).length, 4);
  assert.equal(checked.length, 3);
  assert(checked.every(([, version]) => version === base.version));
  assert.deepEqual(manifest.platforms['darwin-aarch64'], manifest.platforms['darwin-x86_64']);
  assert.equal(
    manifest.platforms['windows-x86_64'].url,
    'https://github.com/gaon12/prism/releases/download/v0.2.0/Prism_0.2.0_x64-setup.exe',
  );
  assert.equal(manifest.notes, base.notes);
});

test('partial manifests are explicit and an entirely missing release always fails', () => {
  const readSignature = (file) => (file.endsWith('.exe') ? 'synthetic' : null);
  assert.throws(() => buildManifest({ ...base, readSignature }), /Missing/);
  const manifest = buildManifest({ ...base, readSignature, allowPartial: true });
  assert.deepEqual(Object.keys(manifest.platforms), ['windows-x86_64']);
  assert.throws(
    () => buildManifest({ ...base, readSignature: () => null, allowPartial: true }),
    /No verified/,
  );
  assert.throws(
    () => buildManifest({ ...base, verifyArtifact: undefined }),
    /verification is required/,
  );
  assert.throws(
    () =>
      buildManifest({
        ...base,
        verifyArtifact: () => {
          throw new Error('bad signature');
        },
      }),
    /bad signature/,
  );
});

test('versions, repository paths and output metadata are bounded before URLs are produced', () => {
  for (const version of ['../escape', '01.2.3', '1.2', '1.2.3-01', '1.2.3+'])
    assert.throws(() => updateFiles(version));
  for (const repo of [
    'https://github.com/gaon12/prism',
    'gaon12/../prism',
    'gaon12/prism?x=1',
    'gaon12/..',
    '-owner/prism',
  ])
    assert.throws(() => buildManifest({ ...base, repo }));
  assert.throws(() => buildManifest({ ...base, pubDate: 'invalid' }));
  assert.throws(() => buildManifest({ ...base, notes: 'x'.repeat(8001) }));
  assert.throws(() => buildManifest({ ...base, readSignature: () => 'x'.repeat(16385) }));
  assert.throws(
    () => buildManifest({ ...base, readSignature: () => 'x'.repeat(16384) }),
    /app limit/,
  );
  const manifest = buildManifest({ ...base, version: '1.2.3-rc.1+build.5' });
  assert(manifest.platforms['windows-x86_64'].url.includes('v1.2.3-rc.1%2Bbuild.5/'));
});

test('actual CLI verifies real public fixtures and refuses to overwrite its manifest', (t) => {
  assert(existsSync(verifier), 'Build tools/update-verifier or set PRISM_UPDATE_VERIFIER first.');
  const f = fixture(t);
  const result = f.run(['--notes', base.notes]);
  assert.equal(result.status, 0, result.stderr);
  const original = readFileSync(f.output, 'utf8');
  assert.equal(Object.keys(JSON.parse(original).platforms).length, 4);
  assert.equal(JSON.parse(original).notes, base.notes);
  assert.notEqual(f.run().status, 0);
  assert.equal(readFileSync(f.output, 'utf8'), original);
});

test('tampering, wrong keys, missing platforms and unbound signatures never produce a manifest', (t) => {
  for (const scenario of ['tamper', 'key', 'missing', 'legacy', 'version']) {
    const f = fixture(t, scenario === 'version' ? '0.3.0' : base.version);
    const environment = {};
    if (scenario === 'tamper') writeFileSync(join(f.directory, f.files[0]), 'changed artifact');
    if (scenario === 'key')
      environment.PRISM_UPDATER_PUBLIC_KEY = readFileSync(
        join(fixtures, 'other-public.pub'),
        'utf8',
      );
    if (scenario === 'missing') rmSync(join(f.directory, f.files[1]));
    if (scenario === 'legacy')
      copyFileSync(join(fixtures, 'legacy.bin.sig'), join(f.directory, `${f.files[0]}.sig`));
    assert.notEqual(f.run([], environment).status, 0, scenario);
    assert.equal(existsSync(f.output), false, scenario);
  }
});

test('invalid CLI options and directory-valued signatures preserve unrelated output', (t) => {
  const f = fixture(t);
  const sentinel = join(f.directory, 'user.txt');
  writeFileSync(sentinel, 'preserve');
  for (const args of [
    ['--out', sentinel],
    ['--repo', '../escape'],
    ['--version'],
    ['--unknown', 'x'],
    ['--repo', 'a/b', '--repo', 'c/d'],
  ]) {
    assert.notEqual(f.run(args).status, 0);
    assert.equal(readFileSync(sentinel, 'utf8'), 'preserve');
    assert.equal(existsSync(f.output), false);
  }
  rmSync(join(f.directory, `${f.files[0]}.sig`));
  mkdirSync(join(f.directory, `${f.files[0]}.sig`));
  assert.notEqual(f.run().status, 0);
  assert.equal(existsSync(f.output), false);
});
