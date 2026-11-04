#!/usr/bin/env node
// Preflight release identity and write a public-only Tauri build config.
import { execFileSync } from 'node:child_process';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { updateFiles } from './make-latest-json.mjs';
import { msixVersion } from './package-msix.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export function releasePlan({
  platform,
  version,
  ref = '',
  publicKey = '',
  privateKeySet = false,
  msixPackageVersion,
}) {
  updateFiles(version); // canonical bounded SemVer, before constructing paths or tag names
  if (!['windows', 'macos', 'linux'].includes(platform))
    throw new Error('Unknown release platform.');
  if (ref.startsWith('refs/tags/') && ref !== `refs/tags/v${version}`)
    throw new Error('Release tag and checked-out application version do not match.');
  if (typeof publicKey !== 'string' || publicKey.length > 8192)
    throw new Error('Invalid updater public key.');
  publicKey = publicKey.trim();
  if (Boolean(publicKey) !== Boolean(privateKeySet))
    throw new Error(
      'Configure both PRISM_UPDATER_PUBLIC_KEY and the updater signing secret, or neither.',
    );
  if (publicKey) {
    const decoded = Buffer.from(publicKey, 'base64');
    const lines = decoded.toString('utf8').trimEnd().split(/\r?\n/);
    const raw = Buffer.from(lines[1] ?? '', 'base64');
    if (
      decoded.toString('base64') !== publicKey ||
      lines.length !== 2 ||
      !lines[0].startsWith('untrusted comment: ') ||
      raw.length !== 42 ||
      raw.subarray(0, 2).toString() !== 'Ed'
    )
      throw new Error('Invalid Tauri updater public key format.');
  }
  const signed = Boolean(publicKey);
  const targets = { windows: [], macos: ['app', 'dmg'], linux: ['appimage'] }[platform];
  return {
    version,
    signed,
    msixPackageVersion: platform === 'windows' ? msixVersion(version, msixPackageVersion) : '',
    config: {
      bundle: { targets, createUpdaterArtifacts: signed && platform !== 'windows' },
      plugins: { updater: { pubkey: publicKey, requireSignedVersion: true } },
    },
  };
}

export function writeReleaseConfig(args = process.argv.slice(2)) {
  const options = new Map();
  for (let index = 0; index < args.length; index++) {
    const name = args[index],
      value = args[++index];
    if (
      !['--platform', '--output'].includes(name) ||
      options.has(name) ||
      !value ||
      value.startsWith('--')
    )
      throw new Error('Expected --platform PLATFORM --output PATH.');
    options.set(name, value);
  }
  if (!options.has('--platform') || !options.has('--output'))
    throw new Error('Platform and output are required.');
  const output = resolve(options.get('--output'));
  if (/[\r\n]/.test(output)) throw new Error('Invalid build-config output path.');
  execFileSync(process.execPath, [resolve(root, 'scripts/bump-version.mjs'), '--check'], {
    stdio: 'pipe',
    windowsHide: true,
  });
  const version = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8')).version;
  const plan = releasePlan({
    platform: options.get('--platform'),
    version,
    ref: process.env.GITHUB_REF ?? '',
    publicKey: process.env.PRISM_UPDATER_PUBLIC_KEY ?? '',
    privateKeySet: Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY?.trim()),
    msixPackageVersion: process.env.PRISM_MSIX_PACKAGE_VERSION || undefined,
  });
  writeFileSync(output, JSON.stringify(plan.config, null, 2) + '\n', { flag: 'wx' });
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `version=${plan.version}\nsigned=${plan.signed}\nconfig=${output}\nmsix_version=${plan.msixPackageVersion}\n`,
    );
  console.log(
    `Prism ${version}: ${plan.signed ? 'version-bound updater signatures required' : 'unsigned build; self-install updates disabled'}.`,
  );
  return plan;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    writeReleaseConfig();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
