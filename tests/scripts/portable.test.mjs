import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import {
  loadPackageNotices,
  noticeNames,
  writePackageNotices,
} from '../../scripts/package-notices.mjs';
import { packagingDirectory, seedNotices } from './packaging-fixture.mjs';

function executable(directory) {
  const bin = join(directory, 'bin');
  mkdirSync(bin);
  const bytes = Buffer.alloc(4096);
  bytes.write('MZ');
  bytes.writeUInt32LE(128, 0x3c);
  bytes.writeUInt32LE(0x4550, 128);
  bytes.writeUInt16LE(0x8664, 132);
  const path = join(bin, 'prism.exe');
  writeFileSync(path, bytes);
  return { bin, path, bytes };
}

test('copies all built-adjacent notice bytes and rejects missing/empty component assets', (t) => {
  const root = packagingDirectory(t, 'portable-notice-bytes');
  const source = executable(root);
  seedNotices(source.bin);
  const payload = join(root, 'payload');
  mkdirSync(payload);
  writePackageNotices(payload, loadPackageNotices(source.path));
  for (const name of noticeNames)
    assert.deepEqual(
      readFileSync(join(payload, 'notices', name)),
      readFileSync(join(source.bin, 'notices', name)),
    );
  const notice = join(source.bin, 'notices', noticeNames[0]);
  writeFileSync(notice, '');
  assert.throws(() => loadPackageNotices(source.path), /invalid component notice/);
  unlinkSync(notice);
  assert.throws(() => loadPackageNotices(source.path), /ENOENT/);
});

test('does not copy a notice directory linked outside the executable folder', (t) => {
  const root = packagingDirectory(t, 'portable-notice-link');
  const source = executable(root);
  const outside = join(root, 'outside');
  mkdirSync(outside);
  seedNotices(outside);
  symlinkSync(join(outside, 'notices'), join(source.bin, 'notices'), 'junction');
  assert.throws(() => loadPackageNotices(source.path), /real directory beside/);
});

test(
  'actual portable ZIP contains exe, marker and every notice with a valid checksum',
  { skip: process.platform !== 'win32' },
  (t) => {
    const root = packagingDirectory(t, 'portable-archive');
    const source = executable(root);
    const out = join(root, 'output');
    const command = [
      resolve('scripts/package-portable.mjs'),
      '--exe',
      source.path,
      '--out',
      out,
      '--version',
      '0.1.0',
    ];
    const missing = spawnSync(process.execPath, command, { encoding: 'utf8', windowsHide: true });
    assert.notEqual(missing.status, 0);
    assert.equal(existsSync(out), false, 'missing assets fail before creating staging/output');
    seedNotices(source.bin);
    const built = spawnSync(process.execPath, command, { encoding: 'utf8', windowsHide: true });
    assert.equal(built.status, 0, built.stderr);
    const zip = join(out, 'Prism_0.1.0_x64_portable.zip');
    const checksum = createHash('sha256').update(readFileSync(zip)).digest('hex');
    assert.equal(
      readFileSync(`${zip}.sha256`, 'utf8'),
      `${checksum}  Prism_0.1.0_x64_portable.zip\n`,
    );
    const entries = JSON.parse(
      execFileSync(
        'powershell.exe',
        [
          '-NoProfile',
          '-NonInteractive',
          '-Command',
          'Add-Type -AssemblyName System.IO.Compression.FileSystem; $zip=[IO.Compression.ZipFile]::OpenRead($env:PRISM_TEST_PORTABLE_ZIP); try { @($zip.Entries | Where-Object { !$_.FullName.EndsWith("/") } | ForEach-Object { $stream=$_.Open(); $bytes=[IO.MemoryStream]::new(); try { $stream.CopyTo($bytes); @{ name=$_.FullName.Replace("\\","/"); bytes=[Convert]::ToBase64String($bytes.ToArray()) } } finally { $stream.Dispose(); $bytes.Dispose() } }) | ConvertTo-Json -Compress } finally { $zip.Dispose() }',
        ],
        {
          encoding: 'utf8',
          env: { ...process.env, PRISM_TEST_PORTABLE_ZIP: zip },
          windowsHide: true,
        },
      ),
    );
    const files = new Map(
      entries.map((entry) => [
        entry.name.replaceAll('\\', '/'),
        Buffer.from(entry.bytes, 'base64'),
      ]),
    );
    const prefix = 'Prism_0.1.0_x64_portable/';
    assert.deepEqual(
      [...files.keys()].sort(),
      [
        'prism.exe',
        'prism.portable',
        'README-portable.txt',
        ...noticeNames.map((name) => `notices/${name}`),
      ]
        .map((name) => prefix + name)
        .sort(),
    );
    assert.deepEqual(files.get(prefix + 'prism.exe'), source.bytes);
    assert.equal(files.get(prefix + 'prism.portable').length, 0);
    for (const name of noticeNames)
      assert.deepEqual(
        files.get(prefix + `notices/${name}`),
        readFileSync(join(source.bin, 'notices', name)),
      );
  },
);
