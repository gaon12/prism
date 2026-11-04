import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { fillTemplate, msixVersion, sdkTool, xml } from '../../scripts/package-msix.mjs';

test('package versions reject lossy prerelease mapping and enforce explicit Store constraints', () => {
  assert.equal(msixVersion('0.1.0'), '0.1.0.0');
  assert.equal(msixVersion('1.2.3', undefined, true), '1.2.3.0');
  assert.equal(msixVersion('1.2.3-rc.1', '1.2.3.4'), '1.2.3.4');
  for (const version of [
    '01.2.3',
    '1.2',
    '../escape',
    '1.2.3-01',
    '1.2.3-rc.1',
    '1.2.3+build',
    '65536.0.0',
  ]) {
    assert.throws(() => msixVersion(version), version);
  }
  assert.throws(() => msixVersion('0.1.0', undefined, true));
  assert.throws(() => msixVersion('1.2.3', '1.2.3.4', true));
  assert.throws(() => msixVersion('1.2.3', '1.2.03.0'));
});

test('manifest values escape markup and reject unresolved or invalid XML data', () => {
  assert.equal(xml(`A&B<"'>🙂`), 'A&amp;B&lt;&quot;&apos;&gt;🙂');
  assert.equal(
    fillTemplate('<x name="{{NAME}}"/>', { NAME: 'Prism & Co' }),
    '<x name="Prism &amp; Co"/>',
  );
  assert.throws(() => fillTemplate('{{UNKNOWN}}', {}));
  assert.throws(() => fillTemplate('{{bad syntax}}', {}));
  assert.throws(() => xml('\0'));
  assert.throws(() => xml('\uD800'));
});

test('invalid CLI inputs preserve preexisting staging files before any SDK work', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'prism-msix-args-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const out = join(directory, 'output');
  mkdirSync(join(out, 'stage'), { recursive: true });
  const sentinel = join(out, 'stage/user.txt');
  writeFileSync(sentinel, 'preserve');
  for (const args of [
    ['--version', '../../escape'],
    ['--out'],
    ['--identity', 'bad/name'],
    ['--sign', 'secret.pfx'],
  ]) {
    const result = spawnSync(
      process.execPath,
      [resolve('scripts/package-msix.mjs'), '--out', out, ...args],
      { windowsHide: true, encoding: 'utf8' },
    );
    assert.notEqual(result.status, 0);
    assert.equal(readFileSync(sentinel, 'utf8'), 'preserve');
    assert.deepEqual(readdirSync(out), ['stage']);
  }
});

test(
  'actual Windows SDK packaging preserves shared output and roundtrips fixture bytes',
  { skip: process.platform !== 'win32' },
  (t) => {
    const directory = mkdtempSync(join(tmpdir(), 'prism-msix-sdk-'));
    t.after(() => rmSync(directory, { recursive: true, force: true }));
    const out = join(directory, 'output');
    mkdirSync(join(out, 'stage'), { recursive: true });
    writeFileSync(join(out, 'stage/user.txt'), 'preserve shared stage');
    // A small real Windows x64 executable is packaged as inert test data, never executed.
    const fixture = join(process.env.WINDIR, 'System32/where.exe');
    const command = [
      resolve('scripts/package-msix.mjs'),
      '--exe',
      fixture,
      '--version',
      '0.1.0',
      '--out',
      out,
    ];
    let result = spawnSync(process.execPath, command, { windowsHide: true, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(join(out, 'stage/user.txt'), 'utf8'), 'preserve shared stage');
    assert(!readdirSync(out).some((name) => name.startsWith('.prism-msix-')));
    const packagePath = join(out, 'Prism_0.1.0_x64.msix');
    const metadata = JSON.parse(readFileSync(`${packagePath}.metadata.json`, 'utf8'));
    const hash = createHash('sha256').update(readFileSync(packagePath)).digest('hex');
    assert.equal(metadata.sha256, hash);
    assert.equal(metadata.signed, false);
    assert.equal(metadata.packageVersion, '0.1.0.0');
    assert(readFileSync(`${packagePath}.sha256`, 'utf8').startsWith(hash));
    const unpacked = join(directory, 'unpacked');
    execFileSync(sdkTool('makeappx.exe'), ['unpack', '/p', packagePath, '/d', unpacked, '/no'], {
      windowsHide: true,
      stdio: 'pipe',
    });
    assert.deepEqual(readFileSync(join(unpacked, 'prism.exe')), readFileSync(fixture));
    const manifest = readFileSync(join(unpacked, 'AppxManifest.xml'), 'utf8');
    assert(manifest.includes('TaskId="PrismStartup" Enabled="false"'));
    assert(manifest.includes('EntryPoint="Windows.FullTrustApplication"'));
    const before = readFileSync(packagePath);
    result = spawnSync(process.execPath, command, { windowsHide: true, encoding: 'utf8' });
    assert.notEqual(result.status, 0, 'an existing output must never be overwritten');
    assert.deepEqual(readFileSync(packagePath), before);
    assert.equal(readFileSync(join(out, 'stage/user.txt'), 'utf8'), 'preserve shared stage');
  },
);
