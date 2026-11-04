import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { checksums, downloadFiles } from '../../scripts/release-checksums.mjs';
async function fixture(t, complete = false) {
  const directory = await mkdtemp(join(tmpdir(), 'prism-release-hashes-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const files = downloadFiles('0.1.0').primary.slice(0, complete ? 4 : 1);
  for (const file of files) await writeFile(join(directory, file), `inert bytes: ${file}`);
  return {
    directory,
    files,
    run: (patch = {}) => checksums({ directory, version: '0.1.0', ...patch }),
  };
}
test('streamed checksums roundtrip, preserve existing sidecars and require complete primary downloads', async (t) => {
  const f = await fixture(t, true);
  assert.equal(await f.run({ write: true, complete: true }), 4);
  const sidecar = join(f.directory, `${f.files[0]}.sha256`);
  const original = await readFile(sidecar, 'utf8');
  assert.equal(await f.run({ complete: true }), 4);
  assert.equal(await f.run({ write: true, complete: true }), 4);
  assert.equal(await readFile(sidecar, 'utf8'), original);
  await rm(join(f.directory, f.files[3]));
  await rm(join(f.directory, `${f.files[3]}.sha256`));
  await assert.rejects(f.run({ complete: true }), /missing/);
});
test('tampering and traversal-valued sidecars fail without creating other checksums', async (t) => {
  const f = await fixture(t, true);
  const sidecar = join(f.directory, `${f.files[0]}.sha256`);
  await writeFile(sidecar, `${'0'.repeat(64)}  ../outside\n`);
  await assert.rejects(f.run({ write: true }), /mismatch/);
  assert.equal((await readdir(f.directory)).filter((name) => name.endsWith('.sha256')).length, 1);
  await rm(sidecar);
  await f.run({ write: true });
  await writeFile(join(f.directory, f.files[0]), 'tampered');
  await assert.rejects(f.run(), /mismatch/);
});
test('unexpected files, directories, stale versions and orphan checksums never become release downloads', async (t) => {
  for (const name of [
    'private-key.txt',
    'Prism_0.0.9_x64-setup.exe',
    'folder',
    'Prism_0.1.0_universal.dmg.sha256',
    'Prism_0.1.0_universal.app.tar.gz.sig',
  ]) {
    const f = await fixture(t);
    if (name === 'folder') await mkdir(join(f.directory, name));
    else await writeFile(join(f.directory, name), 'preserve');
    await assert.rejects(f.run({ write: true }));
    assert.equal((await readdir(f.directory)).length, 2);
  }
  const empty = await fixture(t);
  await writeFile(join(empty.directory, empty.files[0]), '');
  await assert.rejects(empty.run({ write: true }), /Unexpected/);
});
test('actual checksum CLI rejects incomplete sets and malformed options', async (t) => {
  const f = await fixture(t);
  const run = (args) =>
    spawnSync(
      process.execPath,
      [
        resolve('scripts/release-checksums.mjs'),
        '--dir',
        f.directory,
        '--version',
        '0.1.0',
        ...args,
      ],
      { encoding: 'utf8', windowsHide: true },
    );
  assert.equal(run(['--write']).status, 0);
  assert.notEqual(run(['--check', '--complete']).status, 0);
  assert.notEqual(run(['--write', '--check']).status, 0);
  assert.notEqual(run(['--check', '--unknown']).status, 0);
  assert.equal(run(['--check']).status, 0);
});
