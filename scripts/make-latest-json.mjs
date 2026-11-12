#!/usr/bin/env node
// Generate an update manifest only after verifying actual artifact bytes and the bound version.
import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const semver =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;
export function updateFiles(version) {
  if (typeof version !== 'string' || version.length > 128 || !semver.test(version))
    throw new Error('A canonical release version is required.');
  const mac = `Prism_${version}_universal.app.tar.gz`;
  return {
    'windows-x86_64': `Prism_${version}_x64-setup.exe`,
    'darwin-aarch64': mac,
    'darwin-x86_64': mac,
    'linux-x86_64': `Prism_${version}_amd64.AppImage`,
    'linux-aarch64': `Prism_${version}_aarch64.AppImage`,
  };
}
export function buildManifest({
  version,
  repo,
  notes,
  pubDate,
  readSignature,
  verifyArtifact,
  allowPartial = false,
}) {
  const files = updateFiles(version);
  if (
    typeof repo !== 'string' ||
    !/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?\/[A-Za-z0-9._-]{1,100}$/.test(repo) ||
    ['.', '..'].includes(repo.split('/')[1])
  )
    throw new Error('Expected a GitHub owner/repository.');
  if (typeof notes !== 'string' || notes.length > 8000 || /\u0000/.test(notes))
    throw new Error('Release notes must be bounded plain text.');
  if (typeof pubDate !== 'string' || !Number.isFinite(Date.parse(pubDate)))
    throw new Error('A valid publication date is required.');
  if (typeof readSignature !== 'function' || typeof verifyArtifact !== 'function')
    throw new Error('Artifact signature verification is required.');
  const platforms = {};
  const verified = new Map();
  for (const [platform, file] of Object.entries(files)) {
    if (!verified.has(file)) {
      const signature = readSignature(file);
      if (signature == null) {
        if (!allowPartial) throw new Error(`Missing update artifact/signature: ${file}`);
        verified.set(file, null);
      } else {
        if (typeof signature !== 'string' || signature.length > 16384 || !signature.trim())
          throw new Error(`Invalid signature metadata: ${file}`);
        verifyArtifact(file, version);
        verified.set(file, signature.trim());
      }
    }
    const signature = verified.get(file);
    if (!signature) continue;
    platforms[platform] = {
      signature,
      url: `https://github.com/${repo}/releases/download/${encodeURIComponent(`v${version}`)}/${encodeURIComponent(file)}`,
    };
  }
  if (!Object.keys(platforms).length) throw new Error('No verified update artifacts found.');
  const manifest = { version, notes, pub_date: new Date(pubDate).toISOString(), platforms };
  if (Buffer.byteLength(JSON.stringify(manifest, null, 2) + '\n') > 65536)
    throw new Error('Update manifest exceeds the app limit.');
  return manifest;
}

export function writeManifest(args = process.argv.slice(2)) {
  const values = new Map();
  for (let index = 0; index < args.length; index++) {
    const name = args[index];
    if (values.has(name)) throw new Error(`Duplicate manifest option: ${name}`);
    if (name === '--allow-partial') {
      values.set(name, true);
      continue;
    }
    const value = args[++index];
    if (
      !['--dir', '--version', '--repo', '--notes', '--pub-date', '--verifier'].includes(name) ||
      !value ||
      value.startsWith('--')
    )
      throw new Error('Invalid manifest arguments.');
    values.set(name, value);
  }
  if (!values.has('--dir') || !values.has('--version'))
    throw new Error('--dir and --version are required.');
  const directory = resolve(values.get('--dir'));
  const output = join(directory, 'latest.json');
  if (existsSync(output)) throw new Error('Refusing to overwrite an existing latest.json.');
  const key = process.env.PRISM_UPDATER_PUBLIC_KEY;
  if (!key?.trim() || key.length > 8192) throw new Error('The app build public key is required.');
  const verifier = resolve(
    values.get('--verifier') ??
      process.env.PRISM_UPDATE_VERIFIER ??
      join(
        root,
        'tools/update-verifier/target/debug',
        process.platform === 'win32' ? 'prism-update-verifier.exe' : 'prism-update-verifier',
      ),
  );
  if (!lstatSync(verifier).isFile()) throw new Error('Build the update verifier first.');
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(([name]) =>
      [
        'PATH',
        'SYSTEMROOT',
        'WINDIR',
        'TEMP',
        'TMP',
        'HOME',
        'USERPROFILE',
        'LANG',
        'LC_ALL',
      ].includes(name.toUpperCase()),
    ),
  );
  environment.PRISM_UPDATER_PUBLIC_KEY = key;
  const version = values.get('--version');
  const manifest = buildManifest({
    version,
    repo: values.get('--repo') ?? 'gaon12/prism',
    notes: values.get('--notes') ?? `Prism ${version}`,
    pubDate: values.get('--pub-date') ?? new Date().toISOString(),
    allowPartial: values.has('--allow-partial'),
    readSignature(file) {
      const artifact = join(directory, file),
        signature = `${artifact}.sig`;
      if (!existsSync(artifact) || !existsSync(signature)) return null;
      if (
        !lstatSync(artifact).isFile() ||
        !lstatSync(signature).isFile() ||
        lstatSync(signature).size > 16384
      )
        throw new Error(`Invalid update artifact/signature file: ${file}`);
      return readFileSync(signature, 'utf8');
    },
    verifyArtifact(file, expectedVersion) {
      try {
        execFileSync(verifier, ['--version', expectedVersion, '--file', join(directory, file)], {
          env: environment,
          stdio: ['ignore', 'pipe', 'pipe'],
          windowsHide: true,
          timeout: 120000,
          maxBuffer: 65536,
        });
      } catch {
        throw new Error(`Artifact signature/version verification failed: ${file}`);
      }
    },
  });
  writeFileSync(output, JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx' });
  console.log(`latest.json: ${Object.keys(manifest.platforms).join(', ')}`);
  return manifest;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    writeManifest();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
