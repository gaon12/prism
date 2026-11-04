// Actual setup/UI and installed executable, all system writes restricted to an OS-temp sandbox.
// Browser plugin not available: use the repository Playwright/WebView2 CDP workflow.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import { until } from './harness.mjs';

const executable = resolve(
  process.env.PRISM_SETUP_EXE ?? 'installer/src-tauri/target/debug/prism-setup.exe',
);
const expectedApp = resolve(
  process.env.PRISM_SETUP_EXPECTED_EXE ?? 'src-tauri/target/debug/prism.exe',
);
const directory = await mkdtemp(join(tmpdir(), 'prism-setup-e2e-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
const appProfile = join(directory, 'AppProfile');
await mkdir(appProfile);
await writeFile(
  join(appProfile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F9',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    scopes: [],
  }),
);
const digest = async (path) =>
  createHash('sha256')
    .update(await readFile(path))
    .digest('hex');
const expectedHash = await digest(expectedApp);
const expectedVersion = JSON.parse(
  await readFile(resolve('src-tauri/tauri.conf.json'), 'utf8'),
).version;
const exists = async (path) =>
  stat(path).then(
    () => true,
    () => false,
  );
function registration() {
  // Read only this one known key; never write to or remove it during a sandbox test.
  return execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "$key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism'; if(Test-Path -LiteralPath $key){Get-ItemProperty -LiteralPath $key | Select-Object DisplayName,DisplayVersion,InstallLocation,UninstallString,QuietUninstallString,EstimatedSize | ConvertTo-Json -Compress}else{'null'}",
    ],
    { encoding: 'utf8', windowsHide: true },
  ).trim();
}
const realRegistration = registration();
const baseEnvironment = {
  ...process.env,
  PRISM_SETUP_SANDBOX: directory,
  PRISM_DEV_DATA_DIR: appProfile,
};
const active = new Set();
const errors = [];
const warnings = [];
async function environment() {
  const server = createServer();
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return {
    port,
    values: {
      ...baseEnvironment,
      WEBVIEW2_USER_DATA_FOLDER: join(directory, `WebView-${port}`),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
  };
}
function start(path, args, env) {
  const child = spawn(path, args, { env, windowsHide: true, stdio: 'ignore' });
  child.on('error', (error) => {
    child.spawnError = error;
  });
  active.add(child);
  return child;
}
async function exited(child, expected = 0) {
  await until(() => {
    if (child.spawnError) throw child.spawnError;
    return child.exitCode !== null;
  }, 'owned setup process exit');
  active.delete(child);
  if (expected !== null) assert.equal(child.exitCode, expected);
  return child.exitCode;
}
async function connect(port, child, bootstrap = false) {
  await until(async () => {
    if (child?.spawnError) throw child.spawnError;
    if (child?.exitCode !== null && child?.exitCode !== undefined && !bootstrap)
      throw new Error(`Setup exited early: ${child.exitCode}`);
    try {
      return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
    } catch {
      return false;
    }
  }, 'sandbox WebView endpoint');
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'));
    return page;
  }, 'bundled setup page');
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
    else if (message.type() === 'warning') warnings.push(message.text());
  });
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  return { browser, page, invoke };
}
async function temporaryCopies() {
  return new Set(
    (await readdir(tmpdir())).filter((name) => /^prism-uninstall-\d+-\d+$/.test(name)),
  );
}
let current;
let installed;
try {
  for (const invalidRoot of ['', 'relative-sandbox']) {
    const invalid = start(executable, ['/S'], {
      ...baseEnvironment,
      PRISM_SETUP_SANDBOX: invalidRoot,
    });
    await exited(invalid, 1);
  }
  let env = await environment();
  let child = start(executable, [], env.values);
  current = await connect(env.port, child);
  let { page, invoke } = current;
  await page.locator('.setup[data-screen="options"]').waitFor();
  assert.equal(await page.title(), 'Prism Setup');
  assert((await page.locator('body').innerText()).includes('Prism'));
  assert.equal(await page.locator('vite-error-overlay').count(), 0);
  let info = await invoke('setup_info');
  assert.equal(info.version, expectedVersion);
  assert.equal(info.payloadReady, true);
  assert(info.requiredBytes > (await stat(expectedApp)).size + (await stat(executable)).size);
  installed = info.installDir;
  assert(!installed.startsWith('\\\\?\\'), 'the visible path should use normal Windows spelling');
  assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.screenshot({ path: join(evidence, 'setup-options-native.png') });
  const outside = join(tmpdir(), `prism-setup-outside-${Date.now()}`);
  const options = { installDir: installed, desktopShortcut: true, startMenuShortcut: true };
  await assert.rejects(invoke('run_install', { options: { ...options, installDir: outside } }));
  assert.equal(await exists(outside), false);
  await assert.rejects(invoke('finish', { launch: true, installDir: outside }));
  const concurrency = await page.evaluate(async (options) => {
    let attemptedClose = false;
    let closeRequest = Promise.resolve();
    const unlisten = await window.__TAURI__.event.listen('setup://progress', ({ payload }) => {
      if (payload.stage === 'preparing' && !attemptedClose) {
        attemptedClose = true;
        closeRequest = window.__TAURI__.window.getCurrentWindow().close();
      }
    });
    try {
      const results = await Promise.allSettled([
        window.__TAURI_INTERNALS__.invoke('run_install', { options }),
        window.__TAURI_INTERNALS__.invoke('run_install', { options }),
        window.__TAURI_INTERNALS__.invoke('finish', { launch: true }),
      ]);
      await closeRequest;
      return { results, attemptedClose };
    } finally {
      unlisten();
    }
  }, options);
  const { results } = concurrency;
  assert.equal(
    concurrency.attemptedClose,
    true,
    'native close was attempted while setup was active',
  );
  assert.equal(
    results.slice(0, 2).filter((result) => result.status === 'fulfilled').length,
    1,
    JSON.stringify(results),
  );
  assert.equal(results.slice(0, 2).filter((result) => result.status === 'rejected').length, 1);
  assert.equal(results[2].status, 'rejected', 'finish cannot race active writes');
  assert.equal(await digest(join(installed, 'prism.exe')), expectedHash);
  console.log('[setup] native IPC concurrency and file verification passed');
  // Exercise the visible flow as well as raw IPC rejection.
  await page.locator('#install').click();
  await page.locator('.setup[data-screen="done"]').waitFor();
  assert.equal(
    await page.locator('#done-detail').evaluate((element) => element.classList.contains('error')),
    false,
  );
  assert.equal(await page.locator('#track').getAttribute('aria-valuenow'), '100');
  await page.screenshot({ path: join(evidence, 'setup-done-native.png') });
  await page.locator('#launch').uncheck();
  await page.locator('#finish').click();
  await exited(child);
  await current.browser.close();
  current = null;
  assert(await exists(join(directory, 'Desktop/Prism.lnk')));
  assert(await exists(join(directory, 'StartMenu/Prism.lnk')));
  await writeFile(join(installed, 'personal.txt'), 'preserved user fixture');
  const roaming = join(directory, 'RoamingAppData/app.prism.desktop');
  await mkdir(roaming, { recursive: true });
  await writeFile(join(roaming, 'settings.json'), 'preserved settings fixture');
  const competitors = [
    start(executable, ['/S', '/UPDATE'], baseEnvironment),
    start(executable, ['/S', '/UPDATE'], baseEnvironment),
  ];
  const exitCodes = await Promise.all(competitors.map((child) => exited(child, null)));
  assert.deepEqual(
    exitCodes.sort(),
    [0, 1],
    'independent setup processes cannot mutate one folder together',
  );
  assert.equal(await readFile(join(installed, 'personal.txt'), 'utf8'), 'preserved user fixture');
  assert.equal(await digest(join(installed, 'prism.exe')), expectedHash);

  // Actual /R launches the installed app, with all app data/clipboard/model access isolated.
  env = await environment();
  child = start(executable, ['/S', '/UPDATE', '/R', '/ARGS', '--setup-fixture-arg'], env.values);
  await exited(child);
  current = await connect(env.port, null, true);
  assert.equal(await current.page.title(), 'Prism');
  const runtime = await current.invoke('runtime_info');
  assert.equal(runtime.installKind, 'isolated');
  assert.equal(runtime.startupError, null);
  const launchedCommandLine = execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      'Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $env:PRISM_TEST_APP } | Select-Object -ExpandProperty CommandLine',
    ],
    {
      encoding: 'utf8',
      windowsHide: true,
      env: { ...process.env, PRISM_TEST_APP: join(installed, 'prism.exe') },
    },
  );
  assert(
    launchedCommandLine.includes('--setup-fixture-arg'),
    '/ARGS reaches the installed application',
  );
  const appClosed = current.page.waitForEvent('close');
  try {
    await current.invoke('quit_app');
  } catch (cause) {
    assert.match(String(cause), /Target page, context or browser has been closed/);
  }
  await appClosed;
  await until(() => !current.browser.isConnected(), 'isolated launched app exits');
  await current.browser.close();
  current = null;
  console.log('[setup] actual installed app launch and shutdown passed');

  // Bootstrap from the actual installed uninstaller, then interact with its owned temp copy.
  const beforeCopies = await temporaryCopies();
  env = await environment();
  child = start(join(installed, 'uninstall.exe'), [], env.values);
  current = await connect(env.port, child, true);
  ({ page, invoke } = current);
  await page.locator('.setup[data-screen="uninstall"]').waitFor();
  const copies = [...(await temporaryCopies())].filter((name) => !beforeCopies.has(name));
  assert.equal(copies.length, 1);
  const copyDirectory = join(tmpdir(), copies[0]);
  await page.screenshot({ path: join(evidence, 'setup-uninstall-native.png') });
  await page.locator('#uninstall-form button[type="submit"]').click();
  await page.locator('.setup[data-screen="done"]').waitFor();
  assert.equal(
    await page.locator('#done-detail').evaluate((element) => element.classList.contains('error')),
    false,
  );
  assert.equal(await exists(join(installed, 'prism.exe')), false);
  assert.equal(await readFile(join(installed, 'personal.txt'), 'utf8'), 'preserved user fixture');
  assert.equal(
    await readFile(join(roaming, 'settings.json'), 'utf8'),
    'preserved settings fixture',
  );
  assert.equal(await exists(join(directory, 'uninstall-entry.json')), false);
  await page.locator('#finish').click();
  await current.browser.close();
  current = null;
  await until(
    async () => !(await exists(copyDirectory)),
    'native worker removes only its temporary copy',
  );
  await exited(child);
  // Also cover the non-WebView cleanup path and a custom Unicode install directory.
  const secondInstall = join(directory, '두 번째 설치');
  child = start(executable, ['/S', `/D=${secondInstall}`], baseEnvironment);
  await exited(child);
  assert.equal(await digest(join(secondInstall, 'prism.exe')), expectedHash);
  const beforeSilentCopies = await temporaryCopies();
  const silentUninstall = start(join(secondInstall, 'uninstall.exe'), ['/S'], baseEnvironment);
  const silentPid = silentUninstall.pid;
  await exited(silentUninstall);
  await until(
    async () =>
      !(await exists(join(secondInstall, 'prism.exe'))) &&
      !(await exists(join(directory, 'uninstall-entry.json'))),
    'silent temporary uninstaller completes',
  );
  await until(
    async () =>
      [...(await temporaryCopies())].every(
        (name) => !name.startsWith(`prism-uninstall-${silentPid}-`) || beforeSilentCopies.has(name),
      ),
    'silent native cleanup completes',
  );
  assert.equal(
    await readFile(join(roaming, 'settings.json'), 'utf8'),
    'preserved settings fixture',
  );
  assert.equal(registration(), realRegistration, 'real HKCU registration must remain identical');
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      installedHash: expectedHash,
      registry: 'sandbox JSON; real key unchanged',
      cleanup: 'native worker; no shell',
      appLaunch: 'actual isolated Prism',
      consoleWarnings: warnings,
    }),
  );
} catch (cause) {
  if (current?.page)
    await current.page
      .screenshot({ path: join(evidence, 'setup-failure-native.png') })
      .catch(() => {});
  console.error(JSON.stringify({ directory, installed }));
  throw cause;
} finally {
  if (current) {
    await current.invoke('finish', { launch: false }).catch(() => {});
    await current.browser.close().catch(() => {});
  }
  for (const child of active) if (child.exitCode === null) child.kill();
}
