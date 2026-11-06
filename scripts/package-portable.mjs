#!/usr/bin/env node
// Package an existing Windows x64 binary. No build, signing or publishing occurs.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  createReadStream,
  closeSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadPackageNotices, writePackageNotices } from './package-notices.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const options = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index],
    value = process.argv[index + 1];
  if (
    !['--exe', '--out', '--version'].includes(key) ||
    !value ||
    value.startsWith('--') ||
    options.has(key)
  )
    throw new Error('Use --exe <path>, --out <directory>, and --version <semver>.');
  options.set(key, value);
}
const version =
  options.get('--version') ??
  JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8')).version;
if (
  !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/.test(version)
)
  throw new Error('Version must be a filename-safe semantic version.');
if (process.platform !== 'win32')
  throw new Error('Windows portable packaging requires PowerShell and .NET ZipFile.');
const exe = resolve(options.get('--exe') ?? join(root, 'src-tauri/target/release/prism.exe'));
const out = resolve(options.get('--out') ?? join(root, 'src-tauri/target/release/bundle/portable'));
if (!statSync(exe).isFile()) throw new Error('Executable must be a file.');
const header = Buffer.alloc(4096);
const descriptor = openSync(exe, 'r');
let read;
try {
  read = readSync(descriptor, header, 0, header.length, 0);
} finally {
  closeSync(descriptor);
}
const offset = read >= 64 ? header.readUInt32LE(0x3c) : 0;
if (
  header.toString('ascii', 0, 2) !== 'MZ' ||
  offset < 64 ||
  offset + 6 > read ||
  header.readUInt32LE(offset) !== 0x4550 ||
  header.readUInt16LE(offset + 4) !== 0x8664
)
  throw new Error('Expected a Windows x64 PE executable.');
const notices = loadPackageNotices(exe);

mkdirSync(out, { recursive: true });
const stage = mkdtempSync(join(out, '.prism-portable-'));
const name = `Prism_${version}_x64_portable`;
const payload = join(stage, name);
const temporaryZip = join(stage, `${name}.zip`);
const zip = join(out, `${name}.zip`);
try {
  mkdirSync(payload);
  copyFileSync(exe, join(payload, 'prism.exe'));
  writePackageNotices(payload, notices);
  writeFileSync(join(payload, 'prism.portable'), '');
  writeFileSync(
    join(payload, 'README-portable.txt'),
    [
      `Prism ${version} portable`,
      '',
      '- Keep prism.exe and prism.portable together in a writable folder.',
      '- Prism stores its settings, sessions, clipboard, models, dictionaries, indexes and WebView profile in PrismData beside the executable.',
      '- An unwritable data folder stops startup; Prism does not switch its data to an installed profile.',
      '- Login startup is disabled. To update, replace the app files and preserve PrismData.',
      '- Re-enter saved API keys when moving to another Windows account.',
      '- Requires Microsoft Edge WebView2 Runtime.',
      '',
    ].join('\r\n'),
  );
  execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      'Add-Type -AssemblyName System.IO.Compression.FileSystem\n[System.IO.Compression.ZipFile]::CreateFromDirectory($env:PRISM_PACKAGE_STAGE, $env:PRISM_PACKAGE_ZIP, [System.IO.Compression.CompressionLevel]::Optimal, $true)',
    ],
    {
      env: { ...process.env, PRISM_PACKAGE_STAGE: payload, PRISM_PACKAGE_ZIP: temporaryZip },
      stdio: 'inherit',
      windowsHide: true,
    },
  );
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(temporaryZip)) hash.update(chunk);
  const digest = hash.digest('hex');
  const temporaryHash = join(stage, 'checksum.sha256');
  writeFileSync(temporaryHash, `${digest}  ${basename(zip)}\n`);
  renameSync(temporaryZip, zip);
  renameSync(temporaryHash, `${zip}.sha256`);
  console.log(`${zip}\nSHA-256 ${digest}`);
} finally {
  // Delete only this invocation's verified scratch directory, never a version-derived path.
  const actual = realpathSync(stage),
    parent = realpathSync(out);
  const child = relative(parent, actual);
  if (!child || child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child))
    throw new Error('Refusing cleanup outside the packaging output directory.');
  rmSync(actual, { recursive: true, force: true });
}
