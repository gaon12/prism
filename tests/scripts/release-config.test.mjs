import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { releasePlan } from '../../scripts/release-config.mjs';

const key = readFileSync('tests/fixtures/updates/public.pub', 'utf8');
test('signing configuration is paired and public configs never contain private material', () => {
  const base = { platform: 'windows', version: '0.1.0' };
  assert.equal(releasePlan(base).signed, false);
  assert.equal(releasePlan(base).config.plugins.updater.pubkey, '');
  assert.throws(() => releasePlan({ ...base, publicKey: key }));
  assert.throws(() => releasePlan({ ...base, privateKeySet: true }));
  assert.throws(() => releasePlan({ ...base, publicKey: ' \n' }), /blank/);
  assert.throws(() => releasePlan({ ...base, privateKeySet: true, publicKey: 'invalid' }));
  const plan = releasePlan({ ...base, publicKey: key, privateKeySet: true });
  assert.equal(plan.config.plugins.updater.pubkey, key.trim());
  assert.equal(plan.config.plugins.updater.requireSignedVersion, true);
  assert.equal(plan.config.bundle.createUpdaterArtifacts, false);
  assert.deepEqual(plan.config.bundle.targets, []);
  assert.equal(plan.msixPackageVersion, '0.1.0.0');
});

test('platform bundles, signed updater artifacts and tags agree with the release identity', () => {
  for (const platform of ['macos', 'linux']) {
    const plan = releasePlan({
      platform,
      version: '1.2.3',
      publicKey: key,
      privateKeySet: true,
      ref: 'refs/tags/v1.2.3',
    });
    assert.equal(plan.config.bundle.createUpdaterArtifacts, true);
    assert.deepEqual(
      plan.config.bundle.targets,
      platform === 'macos' ? ['app', 'dmg'] : ['appimage'],
    );
  }
  assert.throws(() =>
    releasePlan({ platform: 'windows', version: '0.1.0', ref: 'refs/tags/v0.2.0' }),
  );
  assert.throws(() => releasePlan({ platform: 'other', version: '0.1.0' }));
  assert.throws(() => releasePlan({ platform: 'windows', version: '1.2.3-rc.1' }));
  assert.equal(
    releasePlan({ platform: 'windows', version: '1.2.3-rc.1', msixPackageVersion: '1.2.3.1' })
      .msixPackageVersion,
    '1.2.3.1',
  );
});

test('actual config preflight preserves existing outputs and refuses mismatched tags', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'prism-release-config-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const output = join(directory, 'build.json'),
    githubOutput = join(directory, 'github-output.txt');
  const env = {
    ...process.env,
    GITHUB_REF: 'refs/heads/main',
    GITHUB_OUTPUT: githubOutput,
    PRISM_UPDATER_PUBLIC_KEY: '',
    TAURI_SIGNING_PRIVATE_KEY: '',
    PRISM_MSIX_PACKAGE_VERSION: '',
  };
  const run = (patch = {}) =>
    spawnSync(
      process.execPath,
      [resolve('scripts/release-config.mjs'), '--platform', 'linux', '--output', output],
      { env: { ...env, ...patch }, encoding: 'utf8', windowsHide: true },
    );
  assert.notEqual(run({ GITHUB_REF: 'refs/tags/v999.0.0' }).status, 0);
  assert.equal(existsSync(output), false);
  assert.equal(run().status, 0);
  const original = readFileSync(output, 'utf8');
  assert.equal(JSON.parse(original).bundle.createUpdaterArtifacts, false);
  assert(readFileSync(githubOutput, 'utf8').includes('signed=false'));
  assert.notEqual(run().status, 0);
  assert.equal(readFileSync(output, 'utf8'), original);
  writeFileSync(output, 'user-owned content');
  assert.notEqual(run().status, 0);
  assert.equal(readFileSync(output, 'utf8'), 'user-owned content');
});
