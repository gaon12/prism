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
// Trust is only added after the user's opt-in in the window (`run_install` with
// `trustPublisher: true`, after `setup_info.trustCertificate` showed who and which certificate).
// Cases:
//   no-opt-in  the normal Install button without opting in adds nothing.
//   opt-in     adds Root + TrustedPublisher; the window uninstall removes both.
//   decline    opted in, but "No" to Windows' own question: installed, nothing trusted.
//   silent     /S and /S /UPDATE never add anything, even for a root IT already added;
//              a silent uninstall leaves that IT root alone.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import { setupDebugPort, until, WEBVIEW_START_TIMEOUT } from './harness.mjs';
import { removeTestSandbox } from './test-sandbox.mjs';

assert.equal(process.platform, 'win32');
for (const name of ['PRISM_SETUP_EXE', 'PRISM_SETUP_EXPECTED_EXE', 'PRISM_SETUP_TRUSTED_CERT'])
  assert(process.env[name], `Supply ${name} explicitly.`);
const executable = resolve(process.env.PRISM_SETUP_EXE);
const certificate = await readFile(resolve(process.env.PRISM_SETUP_TRUSTED_CERT));
// The sandbox identifies a certificate by the SHA-256 of its bytes.
const id = createHash('sha256').update(certificate).digest('hex');
const root = await mkdtemp(join(tmpdir(), 'prism-setup-trust-'));
const exists = (path) =>
  stat(path).then(
    () => true,
    () => false,
  );

// Thumbprints in the real per-user lists. Read only; must be identical before and after.
function realLists() {
  return execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "$ErrorActionPreference='Stop'; 'Root','TrustedPublisher' | ForEach-Object { $_ + ':' + ((Get-ChildItem -LiteralPath ('Cert:\\CurrentUser\\' + $_) | ForEach-Object Thumbprint | Sort-Object) -join ',') }",
    ],
    { encoding: 'utf8', windowsHide: true },
  ).trim();
}
const listsBefore = realLists();

async function trustState(sandbox) {
  const file = join(sandbox, 'trust.json');
  if (!(await exists(file))) return { stores: {}, added: {} };
  return JSON.parse(await readFile(file, 'utf8'));
}
const holds = (state, store) => (state.stores[store] ?? []).includes(id);

const active = new Set();
let browser;
function start(path, args, env) {
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

// Opens the setup window of `child` (started with a debug port) and returns its page.
async function windowOf(child, port) {
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
      .find((candidate) => candidate.url().includes('tauri.localhost'));
    return page;
  }, 'setup page');
  return page;
}
async function closeWindow(child) {
  await browser.close();
  browser = undefined;
  await exited(child);
}

// Full-window install into `installed`; returns setup_info.
// Full-window install into `installed`; returns setup_info. `optIn`: install through the
// backend with the user's opt-in, as the window does once it offers the choice; otherwise the
// visible Install button (no opt-in).
async function windowInstall(sandbox, installed, { optIn = false, extraEnv = {} } = {}) {
  const port = await setupDebugPort();
  const child = start(executable, [`/D=${installed}`], {
    ...process.env,
    PRISM_SETUP_SANDBOX: sandbox,
    ...extraEnv,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  });
  const page = await windowOf(child, port);
  await page.locator('.setup[data-screen="options"]').waitFor();
  const info = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('setup_info'));
  if (optIn) {
    // Declining Windows' question must not fail the install either.
    await page.evaluate(
      (installDir) =>
        window.__TAURI_INTERNALS__.invoke('run_install', {
          options: { installDir, desktopShortcut: false, startMenuShortcut: false },
          trustPublisher: true,
        }),
      installed,
    );
    await page
      .evaluate(() => window.__TAURI_INTERNALS__.invoke('finish', { launch: false }))
      .catch(() => {}); // the window may close before the answer arrives
    await browser.close().catch(() => {});
    browser = undefined;
    await exited(child);
    return info;
  }
  await page.locator('#agree').check();
  await page.locator('#install').click();
  await page.locator('.setup[data-screen="done"]').waitFor({ timeout: 180_000 });
  assert.equal(
    await page.locator('#done-detail').evaluate((node) => node.classList.contains('error')),
    false,
  );
  await page.locator('#launch').uncheck();
  await page.locator('#finish').click();
  await closeWindow(child);
  return info;
}

async function windowUninstall(sandbox, installed) {
  const port = await setupDebugPort();
  const child = start(join(installed, 'uninstall.exe'), [], {
    ...process.env,
    PRISM_SETUP_SANDBOX: sandbox,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  });
  const page = await windowOf(child, port);
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

  // 3. Opted in, then "No" to Windows' question: installed normally, nothing trusted.
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
    results.decline = 'installed, nothing trusted';
  }

  // 4. Silent runs never expand trust, even when IT already added the root; a silent
  //    uninstall leaves that root alone.
  {
    const sandbox = join(root, 'silent');
    const installed = join(sandbox, 'Prism');
    await mkdir(sandbox);
    const env = { ...process.env, PRISM_SETUP_SANDBOX: sandbox };
    await exited(start(executable, ['/S', `/D=${installed}`], env));
    let state = await trustState(sandbox);
    assert(!holds(state, 'Root') && !holds(state, 'TrustedPublisher'), JSON.stringify(state));

    const itRoot = JSON.stringify({ stores: { Root: [id] }, added: {} });
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
  const sandboxRemoved = await removeTestSandbox(root);
  console.log(
    JSON.stringify({ passed: true, root, sandboxRemoved, results, realListsUnchanged: true }),
  );
} finally {
  // Close only this test's own windows and processes.
  if (browser?.isConnected()) {
    const page = browser.contexts().flatMap((context) => context.pages())[0];
    await page
      ?.evaluate(() => window.__TAURI_INTERNALS__.invoke('finish', { launch: false }))
      .catch(() => {});
    await browser.close();
  }
  for (const child of active) if (child.exitCode === null) child.kill();
}
