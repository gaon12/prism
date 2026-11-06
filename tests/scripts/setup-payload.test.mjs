import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { noticeNames } from '../../scripts/package-notices.mjs';
import { verifyInstalledPayload } from '../native/setup-payload.mjs';
import { packagingDirectory, seedNotices } from './packaging-fixture.mjs';

function fixture(t) {
  const root = packagingDirectory(t, 'installed-payload');
  const built = join(root, 'built');
  const installed = join(root, 'installed');
  mkdirSync(built);
  mkdirSync(installed);
  writeFileSync(join(built, 'prism.exe'), 'inert app fixture');
  seedNotices(built);
  copyFileSync(join(built, 'prism.exe'), join(installed, 'prism.exe'));
  mkdirSync(join(installed, 'notices'));
  for (const name of noticeNames)
    copyFileSync(join(built, 'notices', name), join(installed, 'notices', name));
  const files = ['prism.exe', ...noticeNames.map((name) => `notices/${name}`)];
  const record = (names = files) =>
    writeFileSync(join(installed, 'install.json'), JSON.stringify({ files: names }));
  record();
  return { installed, expected: join(built, 'prism.exe'), files, record };
}

test('checks exact built bytes and recorded ownership without treating user files as payload', async (t) => {
  const f = fixture(t);
  writeFileSync(join(f.installed, 'notices', 'user.txt'), 'unrelated user data');
  assert.deepEqual(await verifyInstalledPayload(f.installed, f.expected), f.files);
  assert.equal(
    readFileSync(join(f.installed, 'notices', 'user.txt'), 'utf8'),
    'unrelated user data',
  );
});

test('rejects an exe-only setup, altered notices and missing ownership records', async (t) => {
  const f = fixture(t);
  const path = join(f.installed, 'notices', noticeNames[0]);
  const original = readFileSync(path);
  unlinkSync(path);
  await assert.rejects(verifyInstalledPayload(f.installed, f.expected), { code: 'ENOENT' });
  writeFileSync(path, 'different bytes');
  await assert.rejects(verifyInstalledPayload(f.installed, f.expected), /Changed notice/);
  writeFileSync(path, original);
  f.record(['prism.exe']);
  await assert.rejects(verifyInstalledPayload(f.installed, f.expected), /ownership record/);
});

test('rejects changed app bytes and a portable marker in an installed payload', async (t) => {
  const f = fixture(t);
  writeFileSync(join(f.installed, 'prism.exe'), 'unexpected app');
  await assert.rejects(verifyInstalledPayload(f.installed, f.expected));
  copyFileSync(f.expected, join(f.installed, 'prism.exe'));
  writeFileSync(join(f.installed, 'prism.portable'), '');
  await assert.rejects(verifyInstalledPayload(f.installed, f.expected));
});
