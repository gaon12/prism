// End-to-end signing uses disposable keys and inert bytes; no artifact is executed.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { updateFiles } from '../../scripts/make-latest-json.mjs';
import { downloadFiles } from '../../scripts/release-checksums.mjs';

test('fresh Tauri version-bound signing, key pairing, hashes and complete manifest work together', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'prism-disposable-signing-'));
  const tempRoot = realpathSync(tmpdir());
  t.after(() => {
    const actual = realpathSync(directory);
    assert.equal(dirname(actual), tempRoot);
    assert(basename(actual).startsWith('prism-disposable-signing-'));
    rmSync(directory, { recursive: true, force: true });
  });
  const tauri = resolve('node_modules/@tauri-apps/cli/tauri.js');
  const verifier =
    process.env.PRISM_UPDATE_VERIFIER ??
    resolve(
      'tools/update-verifier/target/debug',
      process.platform === 'win32' ? 'prism-update-verifier.exe' : 'prism-update-verifier',
    );
  assert(existsSync(verifier), 'Build tools/update-verifier or set PRISM_UPDATE_VERIFIER first.');
  const privatePath = join(directory, 'disposable.key');
  const environment = {
    ...process.env,
    TAURI_SIGNING_PRIVATE_KEY: '',
    TAURI_SIGNING_PRIVATE_KEY_PATH: '',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: '',
  };
  // Clap treats even an empty *_PATH environment value as a supplied option.
  // Do not accidentally select both key-content and key-path modes.
  delete environment.TAURI_SIGNING_PRIVATE_KEY_PATH;
  const command = (file, args, env = environment) =>
    spawnSync(process.execPath, [file, ...args], {
      env,
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30000,
    });
  // The generator can print key material. Capture it, never log stdout/stderr on failure.
  assert.equal(
    command(tauri, ['signer', 'generate', '--ci', '--password=', '--write-keys', privatePath])
      .status,
    0,
    'Disposable key generation failed.',
  );
  const publicKey = readFileSync(`${privatePath}.pub`, 'utf8');
  const privateKey = readFileSync(privatePath, 'utf8');
  const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
  const signingEnvironment = {
    ...environment,
    TAURI_SIGNING_PRIVATE_KEY: privateKey,
    PRISM_UPDATER_PUBLIC_KEY: publicKey,
  };
  const config = join(directory, 'build.json');
  const configResult = command(
    resolve('scripts/release-config.mjs'),
    ['--platform', 'windows', '--output', config],
    {
      ...signingEnvironment,
      GITHUB_REF: `refs/tags/v${version}`,
      GITHUB_OUTPUT: '',
      PRISM_MSIX_PACKAGE_VERSION: /[+-]/.test(version) ? `${version.split(/[+-]/)[0]}.1` : '',
    },
  );
  assert.equal(configResult.status, 0, 'Public release-config preflight failed.');
  assert.equal(JSON.parse(readFileSync(config, 'utf8')).plugins.updater.pubkey, publicKey.trim());
  assert(
    !readFileSync(config, 'utf8').includes(privateKey.trim()),
    'Private key must not enter the build config.',
  );
  const output = join(directory, 'downloads');
  mkdirSync(output);
  const updates = [...new Set(Object.values(updateFiles(version)))];
  for (const file of new Set([...downloadFiles(version).primary, ...updates]))
    writeFileSync(join(output, file), `inert release fixture: ${file}\n`);
  for (const file of updates)
    assert.equal(
      command(
        tauri,
        ['signer', 'sign', '--app-version', version, join(output, file)],
        signingEnvironment,
      ).status,
      0,
      'Version-bound fixture signing failed.',
    );
  const hashes = resolve('scripts/release-checksums.mjs');
  assert.equal(
    command(hashes, ['--dir', output, '--version', version, '--write', '--complete']).status,
    0,
    'Checksum collection failed.',
  );
  assert.equal(
    command(hashes, ['--dir', output, '--version', version, '--check', '--complete']).status,
    0,
    'Checksum verification failed.',
  );
  const manifestEnvironment = {
    ...environment,
    PRISM_UPDATER_PUBLIC_KEY: publicKey,
    PRISM_UPDATE_VERIFIER: verifier,
  };
  assert.equal(
    command(
      resolve('scripts/make-latest-json.mjs'),
      ['--dir', output, '--version', version],
      manifestEnvironment,
    ).status,
    0,
    'Verified manifest creation failed.',
  );
  const manifest = JSON.parse(readFileSync(join(output, 'latest.json'), 'utf8'));
  assert.equal(manifest.version, version);
  assert.equal(Object.keys(manifest.platforms).length, 4);
  const before = readFileSync(join(output, 'latest.json'), 'utf8');
  // Cryptographic verification still fails if bytes change after collection.
  writeFileSync(join(output, updates[0]), 'tampered');
  assert.notEqual(
    spawnSync(verifier, ['--version', version, '--file', join(output, updates[0])], {
      env: manifestEnvironment,
      windowsHide: true,
    }).status,
    0,
  );
  assert.equal(readFileSync(join(output, 'latest.json'), 'utf8'), before);
  assert(existsSync(privatePath)); // t.after removes the entire verified owned directory.
});
