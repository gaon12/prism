#!/usr/bin/env node
// Build a Windows x64 MSIX without deleting shared staging/output folders or installing it.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  constants,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;
export function msixVersion(version, explicit, store = false) {
  if (typeof version !== 'string' || version.length > 128 || !SEMVER.test(version)) {
    throw new Error('App version must be canonical SemVer.');
  }
  if (/[+-]/.test(version) && !explicit) {
    throw new Error(
      'Prerelease/build versions require --package-version to avoid identity collisions.',
    );
  }
  const value = explicit ?? `${version}.0`;
  if (!/^(0|[1-9]\d*)(\.(0|[1-9]\d*)){3}$/.test(value)) {
    throw new Error('Package version must contain four canonical integers.');
  }
  const parts = value.split('.').map(Number);
  if (
    parts.some((part) => !Number.isInteger(part) || part > 65535) ||
    (store && (parts[0] === 0 || parts[3] !== 0))
  ) {
    throw new Error(
      'Package version fields must fit 0..65535; Store versions need major > 0 and revision 0.',
    );
  }
  return value;
}
export function xml(text) {
  const value = String(text);
  if (/[\u0000-\u0008\u000B\u000C\u000E-\u001F\uD800-\uDFFF\uFFFE\uFFFF]/u.test(value))
    throw new Error('Invalid XML character.');
  return value.replace(
    /[&<>"']/g,
    (character) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' })[character],
  );
}
export function fillTemplate(template, values) {
  const result = template.replace(/\{\{(\w+)\}\}/g, (_, key) => {
    if (!Object.hasOwn(values, key)) throw new Error(`Unknown manifest value {{${key}}}`);
    return xml(values[key]);
  });
  if (result.includes('{{')) throw new Error('Unresolved manifest value.');
  return result;
}
export function sdkTool(name) {
  const bin = join(
    process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)',
    'Windows Kits',
    '10',
    'bin',
  );
  const versions = existsSync(bin)
    ? readdirSync(bin).filter((name) => /^\d+(\.\d+){3}$/.test(name))
    : [];
  versions.sort((a, b) => {
    const x = a.split('.').map(Number),
      y = b.split('.').map(Number);
    for (let index = 0; index < 4; index++) if (x[index] !== y[index]) return y[index] - x[index];
    return 0;
  });
  for (const version of versions) {
    const candidate = join(bin, version, 'x64', name);
    if (existsSync(candidate)) return candidate;
  }
  throw new Error(`${name} is missing; install the Windows SDK.`);
}
function executable(path) {
  if (!statSync(path).isFile()) throw new Error('Executable must be a file.');
  const bytes = readFileSync(path);
  const offset = bytes.length >= 64 ? bytes.readUInt32LE(0x3c) : 0;
  if (
    bytes.toString('ascii', 0, 2) !== 'MZ' ||
    offset < 64 ||
    offset + 6 > bytes.length ||
    bytes.readUInt32LE(offset) !== 0x4550 ||
    bytes.readUInt16LE(offset + 4) !== 0x8664
  ) {
    throw new Error('Expected a Windows x64 PE executable.');
  }
}
function options(args) {
  const values = new Map();
  const pairs = [
    '--exe',
    '--out',
    '--version',
    '--package-version',
    '--publisher',
    '--publisher-name',
    '--identity',
    '--sign-thumbprint',
  ];
  for (let index = 0; index < args.length; index++) {
    const key = args[index];
    if (values.has(key)) throw new Error(`Duplicate option: ${key}`);
    if (key === '--store') {
      values.set(key, true);
      continue;
    }
    const value = args[++index];
    if (!pairs.includes(key) || !value || value.startsWith('--'))
      throw new Error('Invalid MSIX packaging arguments.');
    values.set(key, value);
  }
  return values;
}
export function packageMsix(args = process.argv.slice(2)) {
  const values = options(args);
  const appVersion =
    values.get('--version') ??
    JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8')).version;
  const packageVersion = msixVersion(
    appVersion,
    values.get('--package-version'),
    values.has('--store'),
  );
  const identity = values.get('--identity') ?? 'Prism.Desktop';
  const publisher = values.get('--publisher') ?? 'CN=Prism';
  const publisherName = values.get('--publisher-name') ?? 'Prism';
  if (!/^[A-Za-z0-9][A-Za-z0-9.-]{1,48}[A-Za-z0-9]$/.test(identity))
    throw new Error('Invalid MSIX identity name.');
  for (const value of [publisher, publisherName]) {
    if (!value.trim() || value.length > 512 || /[\r\n]/.test(value))
      throw new Error('Invalid publisher value.');
    xml(value);
  }
  const thumbprint = values.get('--sign-thumbprint')?.replace(/[\s:]/g, '');
  if (thumbprint && !/^[A-Fa-f0-9]{40}$/.test(thumbprint))
    throw new Error('Expected a certificate SHA-1 thumbprint.');
  if (process.platform !== 'win32') throw new Error('MSIX packaging requires Windows and its SDK.');
  const exe = resolve(values.get('--exe') ?? join(root, 'src-tauri/target/release/prism.exe'));
  const out = resolve(values.get('--out') ?? join(root, 'src-tauri/target/release/bundle/msix'));
  executable(exe);
  const makeappx = sdkTool('makeappx.exe');
  const signer = thumbprint ? sdkTool('signtool.exe') : null;
  const fileName = `Prism_${appVersion}_x64.msix`;
  const target = join(out, fileName);
  const outputs = [target, `${target}.sha256`, `${target}.metadata.json`];
  if (outputs.some(existsSync))
    throw new Error('Output already exists. Choose a new output directory.');
  const manifest = fillTemplate(
    readFileSync(join(root, 'installer/msix/AppxManifest.template.xml'), 'utf8'),
    {
      IDENTITY_NAME: identity,
      PUBLISHER: publisher,
      PUBLISHER_DISPLAY_NAME: publisherName,
      VERSION: packageVersion,
    },
  );
  const logos = ['StoreLogo.png', 'Square150x150Logo.png', 'Square44x44Logo.png'];
  for (const name of logos)
    if (!statSync(join(root, 'src-tauri/icons', name)).isFile())
      throw new Error('Missing MSIX logo.');
  mkdirSync(out, { recursive: true });
  const resolvedOut = realpathSync(out);
  const workspace = mkdtempSync(join(resolvedOut, '.prism-msix-'));
  const ownedName = basename(workspace);
  try {
    const content = join(workspace, 'content');
    mkdirSync(join(content, 'Assets'), { recursive: true });
    copyFileSync(exe, join(content, 'prism.exe'));
    for (const name of logos)
      copyFileSync(join(root, 'src-tauri/icons', name), join(content, 'Assets', name));
    writeFileSync(join(content, 'AppxManifest.xml'), manifest, { flag: 'wx' });
    const built = join(workspace, 'package.msix');
    execFileSync(makeappx, ['pack', '/d', content, '/p', built, '/no'], {
      stdio: 'pipe',
      windowsHide: true,
    });
    if (signer) {
      execFileSync(signer, ['sign', '/fd', 'SHA256', '/s', 'My', '/sha1', thumbprint, built], {
        stdio: 'pipe',
        windowsHide: true,
      });
      execFileSync(signer, ['verify', '/pa', built], { stdio: 'pipe', windowsHide: true });
    }
    const hash = createHash('sha256').update(readFileSync(built)).digest('hex');
    // Exclusive copies preserve prior outputs, including a file created during packaging.
    copyFileSync(built, target, constants.COPYFILE_EXCL);
    writeFileSync(`${target}.sha256`, `${hash}  ${fileName}\n`, { flag: 'wx' });
    const metadata = {
      appVersion,
      packageVersion,
      identity,
      publisher,
      signed: Boolean(signer),
      storeVersionRules: values.has('--store'),
      sha256: hash,
    };
    writeFileSync(`${target}.metadata.json`, JSON.stringify(metadata, null, 2) + '\n', {
      flag: 'wx',
    });
    console.log(
      `MSIX: ${target} (${signer ? 'signed' : 'unsigned inspection artifact; sign or use Store distribution before installing'})`,
    );
    return { target, ...metadata };
  } finally {
    // Never delete a shared 'stage' directory or follow a substituted reparse point.
    if (existsSync(workspace)) {
      const resolved = realpathSync(workspace);
      const rel = relative(resolvedOut, resolved);
      if (
        lstatSync(workspace).isSymbolicLink() ||
        rel !== ownedName ||
        rel.startsWith(`..${sep}`) ||
        !ownedName.startsWith('.prism-msix-')
      ) {
        throw new Error('Private MSIX workspace changed; cleanup refused.');
      }
      rmSync(workspace, { recursive: true, force: true });
    }
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    packageMsix();
  } catch (error) {
    console.error(error.code ? `MSIX packaging failed (${error.code}).` : error.message);
    process.exitCode = 1;
  }
}
