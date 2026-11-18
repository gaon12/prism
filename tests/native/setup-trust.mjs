// Publisher-certificate trust, end to end with the real setup in its sandbox.
//
// The sandbox replaces the Windows certificate lists with `trust.json` (see
// installer/src-tauri/src/trust.rs), so this test never changes the real lists; it checks that
// at the start and the end. No installed app is launched.
//
// Needs a setup built with a certificate:
//   PRISM_SETUP_EXE=...prism-setup.exe  PRISM_SETUP_EXPECTED_EXE=...prism.exe
//   PRISM_SETUP_TRUSTED_CERT=...the .cer it was built with   node tests/native/setup-trust.mjs
//
// Trust is only added after the real identity dialog and its unchecked opt-in.
// No IPC response is replaced; setup_info and the embedded UI source are read before UI input.
// Cases:
//   no-opt-in  the normal Install button without opting in adds nothing.
//   opt-in     adds Root + TrustedPublisher; the window uninstall removes both.
//   decline    FileStores sandbox simulation of a rejected root request: installed, nothing
//              trusted. This does not interact with or prove a real Windows consent dialog.
//   silent     /S and /S /UPDATE never add anything, even for a root IT already added;
//              a silent uninstall leaves that IT root alone.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash, randomUUID, X509Certificate } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, mkdtemp, readFile, realpath, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { isAbsolute, join, relative, resolve, sep, toNamespacedPath } from 'node:path';
import { chromium, expect } from '@playwright/test';
import { setupDebugPort, until, WEBVIEW_START_TIMEOUT } from './harness.mjs';
import { removeTestSandbox } from './test-sandbox.mjs';

assert.equal(process.platform, 'win32');
for (const name of ['PRISM_SETUP_EXE', 'PRISM_SETUP_EXPECTED_EXE', 'PRISM_SETUP_TRUSTED_CERT'])
  assert(process.env[name], `Supply ${name} explicitly.`);
const executable = resolve(process.env.PRISM_SETUP_EXE);
const expectedApp = resolve(process.env.PRISM_SETUP_EXPECTED_EXE);
const certificatePath = resolve(process.env.PRISM_SETUP_TRUSTED_CERT);
for (const path of [executable, expectedApp, certificatePath]) {
  const metadata = await lstat(path);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), 'Inputs must be regular files.');
}
// Read only the supplied public DER certificate, never a private key or certificate store.
const certificate = await readFile(certificatePath);
const x509 = new X509Certificate(certificate);
assert.deepEqual(x509.raw, certificate, 'Supply the public DER .cer built into this setup.');
// The sandbox identifies a certificate by the SHA-256 of its bytes.
const id = createHash('sha256').update(certificate).digest('hex');
const thumbprint = createHash('sha1').update(certificate).digest('hex').toUpperCase();
assert.equal(x509.fingerprint256.replaceAll(':', '').toLowerCase(), id);
assert.equal(x509.fingerprint.replaceAll(':', ''), thumbprint);
// Signing's new-certificate recipe has a CN. Windows CERT_NAME_SIMPLE_DISPLAY_TYPE
// chooses CN before OU, O or email; compare that parsed X509 value, not a formatted DN.
const certificateSubject = x509.toLegacyObject().subject.CN;
assert.equal(typeof certificateSubject, 'string', 'The signing certificate must have one CN.');
assert(certificateSubject.length > 0);
const expectedIdentity = {
  subject: certificateSubject,
  sha256: x509.fingerprint256,
  thumbprint,
  validUntil: new Date(x509.validTo).toISOString().slice(0, 10),
};
const setupUiSha256 = createHash('sha256')
  .update(await readFile(new URL('../../installer/ui/setup.js', import.meta.url)))
  .digest('hex');
const expectedVersion = JSON.parse(
  await readFile(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
).version;
const root = await realpath(await mkdtemp(join(tmpdir(), 'prism-setup-trust-')));
const ownerPath = join(root, 'fixture-owner.json');
const nonce = randomUUID();
await writeFile(ownerPath, JSON.stringify({ nonce, pid: process.pid }), { flag: 'wx' });
function pathKey(path) {
  return toNamespacedPath(resolve(path)).toLowerCase();
}
function inside(parent, path) {
  const suffix = relative(toNamespacedPath(resolve(parent)), toNamespacedPath(resolve(path)));
  assert(
    suffix && suffix !== '..' && !suffix.startsWith(`..${sep}`) && !isAbsolute(suffix),
    'Path must stay inside the nonce-owned sandbox.',
  );
}
async function ownedRoot() {
  assert.deepEqual(JSON.parse(await readFile(ownerPath, 'utf8')), { nonce, pid: process.pid });
  assert.equal(await realpath(root), root);
}
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
const expectedAppSha256 = await hash(expectedApp);
const exists = (path) =>
  stat(path).then(
    () => true,
    () => false,
  );

// Read-only .NET stores avoid PowerShell provider/module-path dependencies. Return
// only a digest of sorted thumbprints, so assertion failures never print an inventory.
function realLists() {
  const fingerprint = execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
function Read-StoreThumbprints([string] $storeName) {
  $store = [System.Security.Cryptography.X509Certificates.X509Store]::new(
    $storeName, [System.Security.Cryptography.X509Certificates.StoreLocation]::CurrentUser)
  try {
    try {
      $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly -bor
        [System.Security.Cryptography.X509Certificates.OpenFlags]::OpenExistingOnly)
    } catch {
      $failure = $_.Exception
      while ($failure.InnerException) { $failure = $failure.InnerException }
      $code = '{0:X8}' -f $failure.HResult
      # Only explicit CRYPT_E_NOT_FOUND / ERROR_FILE_NOT_FOUND mean an empty known store.
      if (($failure -is [System.Security.Cryptography.CryptographicException]) -and
          ($code -in @('80092004', '80070002'))) { return }
      throw "CurrentUser/$storeName could not be read (HRESULT=$code)."
    }
    $certificates = $store.Certificates
    try {
      $thumbprints = [string[]] @($certificates | ForEach-Object { $_.Thumbprint.ToUpperInvariant() })
      [Array]::Sort($thumbprints, [StringComparer]::Ordinal)
      return $thumbprints
    } finally {
      foreach ($certificate in $certificates) { $certificate.Dispose() }
    }
  } finally {
    $store.Close()
    $store.Dispose()
  }
}
$entries = foreach ($storeName in @('Root', 'TrustedPublisher', 'My')) {
  $thumbprints = @(Read-StoreThumbprints $storeName)
  $storeName + ':' + ($thumbprints -join ',')
}
$digest = [System.Security.Cryptography.SHA256]::Create()
try {
  $bytes = [Text.Encoding]::UTF8.GetBytes(($entries -join ([char]10)))
  [BitConverter]::ToString($digest.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
} finally { $digest.Dispose() }
`,
    ],
    { encoding: 'utf8', windowsHide: true },
  ).trim();
  assert(/^[0-9a-f]{64}$/u.test(fingerprint), 'Read-only certificate snapshot must yield SHA-256.');
  return fingerprint;
}
const listsBefore = realLists();

async function trustState(sandbox) {
  await ownedRoot();
  inside(root, sandbox);
  const file = join(sandbox, 'trust.json');
  if (!(await exists(file))) return { stores: {}, added: {} };
  return JSON.parse(await readFile(file, 'utf8'));
}
const holds = (state, store) => (state.stores[store] ?? []).includes(id);

const active = new Set();
let browser;
let current;
function start(path, args, env) {
  assert(env.PRISM_SETUP_SANDBOX, 'Every setup process must use the private sandbox.');
  inside(root, env.PRISM_SETUP_SANDBOX);
  assert(!args.some((arg) => /^\/R(?:$|[:=])/i.test(arg)), 'Never launch the payload.');
  const child = spawn(path, args, { env, windowsHide: true, stdio: 'ignore' });
  child.on('error', (error) => (child.spawnError = error));
  active.add(child);
  return child;
}
async function exited(child) {
  await until(
    () => {
      if (child.spawnError) throw child.spawnError;
      return child.exitCode !== null;
    },
    'setup exit',
    180_000,
  );
  assert.equal(child.exitCode, 0);
  active.delete(child);
}
async function freePort(port) {
  const reservation = createServer();
  await new Promise((done, reject) => {
    reservation.once('error', reject);
    reservation.listen(port, '127.0.0.1', done);
  });
  await new Promise((done) => reservation.close(done));
}

// Opens the setup window of `child` (started with a debug port) and returns its page.
async function windowOf(child, port, sandbox, installed, mode) {
  current = { verified: false };
  await until(
    async () => {
      if (child.spawnError) throw child.spawnError;
      try {
        return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
      } catch {
        return false;
      }
    },
    'setup WebView',
    WEBVIEW_START_TIMEOUT,
  );
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => {
        try {
          return new URL(candidate.url()).hostname === 'tauri.localhost';
        } catch {
          return false;
        }
      });
    return page;
  }, 'setup page');
  current.page = page;
  // Read-only IPC first. No controls, screenshots or mutating IPC until all gates pass.
  const info = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('setup_info'));
  await ownedRoot();
  assert.equal(pathKey(info.installDir), pathKey(installed));
  for (const path of [info.installDir, info.defaultDir, info.logPath]) {
    assert(path, 'The setup must report its sandbox paths.');
    inside(sandbox, path);
  }
  inside(sandbox, await realpath(info.logPath));
  const log = await lstat(info.logPath);
  assert(log.isFile() && !log.isSymbolicLink());
  assert.equal(info.version, expectedVersion);
  assert.equal(info.ui, 'full');
  assert.equal(info.relaunch, false);
  assert.equal(info.mode, mode);
  assert.equal(info.payloadReady, true);
  assert.deepEqual(
    info.trustCertificate,
    expectedIdentity,
    'Descriptor must match the public DER.',
  );
  const cdp = await page.context().newCDPSession(page);
  try {
    const scripts = [];
    cdp.on('Debugger.scriptParsed', (script) => {
      if (/\/setup\.js(?:[?#]|$)/.test(script.url)) scripts.push(script);
    });
    await cdp.send('Debugger.enable');
    await until(() => scripts.length > 0, 'embedded trust UI script');
    const { scriptSource } = await cdp.send('Debugger.getScriptSource', {
      scriptId: scripts[0].scriptId,
    });
    assert.equal(
      createHash('sha256').update(scriptSource).digest('hex'),
      setupUiSha256,
      'Rebuild setup with the current publisher-consent UI before running this test.',
    );
    await cdp.send('Debugger.disable');
  } finally {
    await cdp.detach();
  }
  current.verified = true;
  return { page, info };
}
async function closeWindow(child) {
  await browser.close();
  browser = undefined;
  current = undefined;
  await exited(child);
}

// Every install uses the real visible controls. Opt-in requires opening the actual
// certificate dialog, comparing its four identity fields, checking and applying the choice.
async function windowInstall(sandbox, installed, { optIn = false, extraEnv = {} } = {}) {
  await ownedRoot();
  inside(sandbox, installed);
  const port = await setupDebugPort();
  await freePort(port);
  const child = start(executable, [`/D=${installed}`], {
    ...process.env,
    PRISM_SETUP_SANDBOX: sandbox,
    ...extraEnv,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  });
  const { page, info } = await windowOf(child, port, sandbox, installed, 'install');
  await page.locator('.setup[data-screen="options"]').waitFor();
  await expect(page.locator('#folder')).toHaveValue(info.installDir);
  await expect(page.locator('#trust-options')).toBeVisible();
  await expect(page.locator('#trust-publisher')).not.toBeChecked();
  if (optIn) {
    await page.locator('#trust-options').click();
    await expect(page.locator('#trust-form')).toBeVisible();
    for (const [field, key] of [
      ['subject', 'subject'],
      ['sha256', 'sha256'],
      ['thumbprint', 'thumbprint'],
      ['valid-until', 'validUntil'],
    ]) {
      const identityField = page.locator('#trust-' + field);
      await identityField.scrollIntoViewIfNeeded();
      await expect(identityField).toBeVisible();
      await expect(identityField).toHaveText(expectedIdentity[key]);
    }
    await expect(page.locator('#trust-publisher')).not.toBeChecked();
    await page.locator('#trust-publisher').check();
    await page.locator('#trust-confirm').click();
    await expect(page.locator('#trust-form')).toBeHidden();
  }
  // Shortcuts are irrelevant to trust and stay inside the sandbox either way.
  await page.locator('#desktop').uncheck();
  await page.locator('#start-menu').uncheck();
  await page.locator('#agree').check();
  await ownedRoot();
  await expect(page.locator('#folder')).toHaveValue(info.installDir);
  await page.locator('#install').click();
  await page.locator('.setup[data-screen="done"]').waitFor({ timeout: 180_000 });
  assert.equal(
    await page.locator('#done-detail').evaluate((node) => node.classList.contains('error')),
    false,
  );
  assert.equal(
    await hash(join(installed, 'prism.exe')),
    expectedAppSha256,
    'The installed app must match the supplied qualified payload.',
  );
  await expect(page.locator('#trust-publisher')).not.toBeChecked();
  await page.locator('#launch').uncheck();
  await page.locator('#finish').click();
  await closeWindow(child);
  return info;
}

async function windowUninstall(sandbox, installed) {
  await ownedRoot();
  inside(sandbox, installed);
  const port = await setupDebugPort();
  await freePort(port);
  const child = start(join(installed, 'uninstall.exe'), [], {
    ...process.env,
    PRISM_SETUP_SANDBOX: sandbox,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  });
  const { page } = await windowOf(child, port, sandbox, installed, 'uninstall');
  await page.locator('.setup[data-screen="uninstall"]').waitFor();
  await page.locator('#uninstall-form button[type="submit"]').click();
  await page.locator('.setup[data-screen="done"]').waitFor({ timeout: 90_000 });
  await page.locator('#finish').click();
  // The bootstrap copy exits by itself; wait for the window process to finish.
  await browser.close().catch(() => {});
  browser = undefined;
  await until(async () => !(await exists(join(installed, 'uninstall.exe'))), 'uninstall done');
}

const results = {};
try {
  // 1. The normal Install button without opting in: nothing is added.
  {
    const sandbox = join(root, 'no-opt-in');
    const installed = join(sandbox, 'Prism');
    await mkdir(sandbox);
    const info = await windowInstall(sandbox, installed);
    // The window gets the identity to show before anyone decides.
    assert.equal(info.trustCertificate.sha256.replaceAll(':', '').toLowerCase(), id);
    assert.match(info.trustCertificate.thumbprint, /^[0-9A-F]{40}$/);
    assert.match(info.trustCertificate.validUntil, /^\d{4}-\d{2}-\d{2}$/);
    assert(info.trustCertificate.subject.length > 0);
    assert(await exists(join(installed, 'prism.exe')));
    const state = await trustState(sandbox);
    assert(!holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));
    results.noOptIn = 'installed, nothing trusted';
  }

  // 2. Opt-in: added to both lists, remembered, and removed again by the window uninstall.
  {
    const sandbox = join(root, 'opt-in');
    const installed = join(sandbox, 'Prism');
    await mkdir(sandbox);
    await windowInstall(sandbox, installed, { optIn: true });
    let state = await trustState(sandbox);
    assert(holds(state, 'Root') && holds(state, 'TrustedPublisher'), JSON.stringify(state));
    assert.deepEqual(state.added[id], ['Root', 'TrustedPublisher']);
    await windowUninstall(sandbox, installed);
    state = await trustState(sandbox);
    assert(!holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));
    assert.equal(state.added[id], undefined);
    results.optIn = 'added then removed';
  }

  // 3. The real UI opts in, then FileStores simulates rejection. This does not
  // operate a real Windows consent dialog or claim its behavior was tested.
  {
    const sandbox = join(root, 'decline');
    const installed = join(sandbox, 'Prism');
    await mkdir(sandbox);
    await windowInstall(sandbox, installed, {
      optIn: true,
      extraEnv: { PRISM_SETUP_SANDBOX_TRUST: 'decline' },
    });
    assert(await exists(join(installed, 'prism.exe')));
    const state = await trustState(sandbox);
    assert(!holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));
    assert.equal(state.added[id], undefined);
    results.decline = 'FileStores sandbox simulated rejection; installed, nothing trusted';
  }

  // 4. Silent runs never expand trust, even when IT already added the root; a silent
  //    uninstall leaves that root alone.
  {
    const sandbox = join(root, 'silent');
    const installed = join(sandbox, 'Prism');
    await mkdir(sandbox);
    const env = { ...process.env, PRISM_SETUP_SANDBOX: sandbox };
    await ownedRoot();
    await exited(start(executable, ['/S', `/D=${installed}`], env));
    let state = await trustState(sandbox);
    assert(!holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));

    const itRoot = JSON.stringify({ stores: { Root: [id] }, added: {} });
    await ownedRoot();
    await writeFile(join(sandbox, 'trust.json'), itRoot);
    await exited(start(executable, ['/S', '/UPDATE', `/D=${installed}`], env));
    state = await trustState(sandbox);
    assert(holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));
    assert.equal(state.added[id], undefined, 'a silent update adds nothing');

    await exited(start(join(installed, 'uninstall.exe'), ['/S'], env));
    await until(async () => !(await exists(join(installed, 'prism.exe'))), 'silent uninstall');
    state = await trustState(sandbox);
    assert(holds(state, 'Root'), 'the root IT added stays');
    results.silent = 'nothing added; IT root kept';
  }

  assert.equal(realLists(), listsBefore, 'the real certificate lists are unchanged');
  await ownedRoot();
  const sandboxRemoved = await removeTestSandbox(root);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      sandboxRemoved,
      results,
      setupUiSha256,
      certificateSha256: id,
      appSha256: expectedAppSha256,
      realListsUnchanged: true,
    }),
  );
} finally {
  // Close only this test's own windows and processes.
  if (browser?.isConnected()) {
    if (current?.verified)
      await current.page
        .evaluate(() => window.__TAURI_INTERNALS__.invoke('finish', { launch: false }))
        .catch(() => {});
    await browser.close();
  }
  for (const child of active) if (child.exitCode === null) child.kill();
  assert.equal(
    realLists(),
    listsBefore,
    'Real Root/TrustedPublisher/My lists must stay unchanged, including on failure.',
  );
}
