// Genuine app processes and setup IPC, isolated in a nonce-owned portable sandbox.
// This covers process closure, not an installed-profile runtime qualification.
// Browser plugin unavailable: existing Playwright/CDP WebView2 harness.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  unlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, toNamespacedPath } from 'node:path';
import { chromium } from '@playwright/test';
import { launch, shutdown, setupDebugPort, until, untilWebView } from './harness.mjs';
import { verifyInstalledPayload } from './setup-payload.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_SETUP_EXE && process.env.PRISM_SETUP_EXPECTED_EXE);
const setup = resolve(process.env.PRISM_SETUP_EXE);
const candidate = resolve(process.env.PRISM_SETUP_EXPECTED_EXE);
for (const path of [setup, candidate]) assert((await lstat(path)).isFile());
const root = await realpath(await mkdtemp(join(tmpdir(), 'prism-setup-running-')));
const nonce = randomUUID();
const ownership = { nonce, pid: process.pid };
const owner = join(root, 'fixture-owner.json');
await writeFile(owner, JSON.stringify(ownership), { flag: 'wx' });
const installation = join(root, 'Installation A');
const other = join(root, 'Unrelated portable B');
const key = (path) => toNamespacedPath(resolve(path)).toLowerCase();
const hash = async (path) => {
  const digest = createHash('sha256');
  for await (const bytes of createReadStream(path)) digest.update(bytes);
  return digest.digest('hex');
};
const setupHash = await hash(setup);
const appHash = await hash(candidate);
const uiHash = await hash(resolve('installer/ui/setup.js'));
function personalRegistrationFingerprint() {
  const snapshot = execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      String.raw`$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $paths=@('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Prism','HKCU:\Software\Microsoft\Windows\CurrentVersion\Run','HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'); foreach($p in $paths){if(Test-Path -LiteralPath $p){if($p.EndsWith('Uninstall\Prism')){Get-ItemProperty -LiteralPath $p | Select-Object DisplayVersion,InstallLocation,UninstallString | ConvertTo-Json -Compress}else{$v=Get-ItemProperty -LiteralPath $p; if($null -ne $v.PSObject.Properties['Prism']){$v.Prism | ConvertTo-Json -Compress}else{'null'}}}else{'null'}}`,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  return createHash('sha256').update(snapshot).digest('hex');
}
const realRegistrationBefore = personalRegistrationFingerprint();
let activeSetup;
let target;
let independent;
const stages = [];
const errors = [];
async function connect(uninstall = false) {
  assert.deepEqual(JSON.parse(await readFile(owner, 'utf8')), ownership);
  const port = await setupDebugPort();
  let executable = setup;
  if (uninstall) {
    // Use the same ownership format enforced by the production temporary-copy guard.
    // mkdir fails rather than reusing any existing directory.
    const temporary = join(tmpdir(), `prism-uninstall-${process.pid}-${Date.now()}`);
    await mkdir(temporary);
    executable = join(temporary, 'uninstall.exe');
    await copyFile(setup, executable);
    await writeFile(join(temporary, 'owner.txt'), 'Prism temporary uninstall copy v1\n', {
      flag: 'wx',
    });
    assert.equal(await hash(executable), setupHash);
  }
  const child = spawn(
    executable,
    uninstall ? ['--from-temp', installation] : [`/D=${installation}`],
    {
      windowsHide: true,
      stdio: 'ignore',
      env: {
        ...process.env,
        PRISM_SETUP_SANDBOX: root,
        WEBVIEW2_USER_DATA_FOLDER: join(root, 'SetupWebView'),
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
      },
    },
  );
  activeSetup = { child };
  await untilWebView(child, port, 'private running-app setup');
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  activeSetup.browser = browser;
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((x) => x.pages())
      .find((x) => /(?:tauri\.localhost|tauri:\/\/localhost)/.test(x.url()));
    return page;
  }, 'bundled setup page');
  page.on('pageerror', (error) => errors.push(error.message));
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  const info = await invoke('setup_info');
  assert.equal(key(info.installDir), key(installation));
  assert.equal(info.ui, 'full');
  assert.equal(info.relaunch, false);
  assert(key(info.logPath).startsWith(`${key(root)}\\`));
  const cdp = await page.context().newCDPSession(page);
  const scripts = [];
  cdp.on('Debugger.scriptParsed', (script) => {
    if (/\/setup\.js(?:[?#]|$)/.test(script.url)) scripts.push(script);
  });
  await cdp.send('Debugger.enable');
  await until(() => scripts.length, 'embedded setup source');
  const { scriptSource } = await cdp.send('Debugger.getScriptSource', {
    scriptId: scripts[0].scriptId,
  });
  assert.equal(createHash('sha256').update(scriptSource).digest('hex'), uiHash);
  await cdp.detach();
  await page.evaluate(async () => {
    window.__runningStages = [];
    await window.__TAURI__.event.listen('setup://progress', ({ payload }) =>
      window.__runningStages.push(payload.stage),
    );
  });
  activeSetup = { ...activeSetup, page, invoke };
  return activeSetup;
}
async function stopSetup() {
  if (!activeSetup) return;
  const value = activeSetup;
  if (value.page && !value.page.isClosed())
    stages.push(...(await value.page.evaluate(() => window.__runningStages)));
  try {
    await value.invoke('finish', { launch: false });
  } catch {
    /* Native window can close before reply. */
  }
  await until(() => value.child.exitCode !== null, 'owned setup exits', 30_000);
  assert.equal(value.child.exitCode, 0);
  await value.browser.close();
  activeSetup = null;
}
async function portable(folder, shortcut) {
  await writeFile(join(folder, 'prism.portable'), '');
  const data = join(folder, 'PrismData');
  await mkdir(data, { recursive: true });
  await writeFile(
    join(data, 'settings.json'),
    JSON.stringify({
      onboardingVersion: 1,
      closeToTray: true,
      startOnLogin: false,
      shortcut,
      scopes: [],
      clipboard: { enabled: false, onboarded: true },
      updates: { autoCheck: false },
    }),
  );
  const app = await launch(data, {
    portable: true,
    productionPortable: true,
    executable: join(folder, 'prism.exe'),
  });
  assert.equal((await app.invoke('runtime_info')).installKind, 'portable');
  assert.equal((await app.invoke('get_settings')).closeToTray, true);
  await app.invoke('window_action', { action: 'close' });
  assert.equal(app.child.exitCode, null, 'Window close must leave the tray process running');
  if (key(folder) === key(installation)) {
    // The real app has already selected immutable portable state.location and data
    // paths. Remove only our marker so setup can maintain its own registered
    // fixture. Never launch the exe again until portable() recreates the marker.
    // No production path override or installer portable-protection bypass is used.
    assert.deepEqual(JSON.parse(await readFile(owner, 'utf8')), ownership);
    await unlink(join(folder, 'prism.portable'));
    assert.equal((await app.invoke('runtime_info')).installKind, 'portable');
  }
  return app;
}
try {
  const first = await connect();
  await first.invoke('run_install', {
    options: { installDir: installation, desktopShortcut: false, startMenuShortcut: false },
    closeRunningApp: false,
  });
  await verifyInstalledPayload(installation, candidate);
  await stopSetup();
  await mkdir(other);
  await copyFile(candidate, join(other, 'prism.exe'));
  independent = await portable(other, 'Alt+Shift+F10');
  target = await portable(installation, 'Alt+Shift+F11');
  const registration = await readFile(join(root, 'uninstall-entry.json'));
  const record = await readFile(join(installation, 'install.json'));
  const repair = await connect();
  await assert.rejects(repair.invoke('run_repair', { installDir: installation }), /APP_RUNNING:/);
  assert.equal(target.child.exitCode, null);
  assert.deepEqual(await readFile(join(installation, 'install.json')), record);
  assert.deepEqual(await readFile(join(root, 'uninstall-entry.json')), registration);
  await repair.invoke('run_repair', { installDir: installation, closeRunningApp: true });
  await until(() => target.child.exitCode !== null, 'repair gracefully closes hidden app', 15_000);
  await target.browser.close();
  target = null;
  assert.equal(independent.child.exitCode, null, 'Other-folder same-name app must survive repair');
  assert.equal(await hash(join(installation, 'prism.exe')), appHash);
  await stopSetup();
  target = await portable(installation, 'Alt+Shift+F11');
  const removal = await connect(true);
  await assert.rejects(removal.invoke('run_uninstall', { removeData: false }), /APP_RUNNING:/);
  assert.equal(target.child.exitCode, null);
  assert.equal(await hash(join(installation, 'prism.exe')), appHash);
  await removal.invoke('run_uninstall', { removeData: false, closeRunningApp: true });
  await until(
    () => target.child.exitCode !== null,
    'uninstall gracefully closes hidden app',
    15_000,
  );
  await target.browser.close();
  target = null;
  assert.equal(independent.child.exitCode, null, 'Other-folder app must survive uninstall');
  await assert.rejects(lstat(join(installation, 'prism.exe')), { code: 'ENOENT' });
  await assert.rejects(lstat(join(root, 'uninstall-entry.json')), { code: 'ENOENT' });
  assert((await lstat(join(installation, 'PrismData'))).isDirectory());
  await stopSetup();
  assert(stages.includes('closing'));
  assert.deepEqual(errors, []);
  assert.equal(
    personalRegistrationFingerprint(),
    realRegistrationBefore,
    'Personal registration must stay unchanged',
  );
  const result = {
    root,
    setupHash,
    appHash,
    uiHash,
    realRegistrationUnchanged: true,
    actualAppProcesses: true,
    closeToTray: true,
    repair: true,
    uninstall: true,
    defaultNoConsentPreserved: true,
    otherFolderPreserved: true,
    personalInstalledProfileUsed: false,
    stages: [...new Set(stages)],
    stageEventCount: stages.length,
    errors,
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} finally {
  for (const app of [target, independent]) if (app) await shutdown(app);
  if (activeSetup) {
    await activeSetup.browser?.close().catch(() => {});
    if (activeSetup.child.exitCode === null) activeSetup.child.kill();
  }
}
