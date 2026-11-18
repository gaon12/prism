// Real setup/WebView2 feature checks, exclusively in a new nonce-owned OS-temp sandbox.
// Browser plugin not available: use the existing Playwright/CDP workflow.
// Never starts the bundled app: no /R, /ARGS or finish(true), and no Explorer or clipboard calls.
// UI source identity and sandbox install/log paths are verified before any UI mutation or capture.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, mkdtemp, readFile, realpath, unlink, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { isAbsolute, join, relative, resolve, sep, toNamespacedPath } from 'node:path';
import { chromium, expect } from '@playwright/test';
import { setupDebugPort, until, untilWebView } from './harness.mjs';
import { verifyInstalledPayload } from './setup-payload.mjs';

assert.equal(process.platform, 'win32', 'This test needs the real Windows setup.');
assert(process.env.PRISM_SETUP_EXE, 'Supply the candidate setup explicitly.');
assert(process.env.PRISM_SETUP_EXPECTED_EXE, 'Supply the actual bundled app explicitly.');
const executable = resolve(process.env.PRISM_SETUP_EXE);
const expectedApp = resolve(process.env.PRISM_SETUP_EXPECTED_EXE);
for (const path of [executable, expectedApp]) {
  const metadata = await lstat(path);
  assert(
    metadata.isFile() && !metadata.isSymbolicLink(),
    'Candidate inputs must be regular files.',
  );
}
const nonce = randomUUID();
const root = await realpath(await mkdtemp(join(tmpdir(), 'prism-setup-features-')));
const ownerPath = join(root, 'fixture-owner.json');
await writeFile(ownerPath, JSON.stringify({ nonce, pid: process.pid }), { flag: 'wx' });
const evidence = join(root, 'evidence');
await mkdir(evidence);
const installA = join(root, 'Installation A');
const installB = join(root, 'Installation B');
const operationTimeout = 180_000; // Same bounded whole-payload budget as the existing native setup suite.
const errors = [],
  warnings = [],
  results = [],
  inputs = {};
const active = new Set();
let current;
function inside(parent, path) {
  // The native path-policy helper returns \\?\ paths; compare both sides in the
  // same Windows namespace, while still rejecting other roots and escaping paths.
  const suffix = relative(toNamespacedPath(resolve(parent)), toNamespacedPath(resolve(path)));
  assert(
    suffix && suffix !== '..' && !suffix.startsWith(`..${sep}`) && !isAbsolute(suffix),
    'Path must stay in the owned sandbox.',
  );
}
function pathKey(path) {
  return toNamespacedPath(resolve(path)).toLowerCase();
}
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
function registration() {
  // Read only this known key. Never writes to or removes real HKCU registration.
  return execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "$ErrorActionPreference='Stop'; $key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism'; if(Test-Path -LiteralPath $key){Get-ItemProperty -LiteralPath $key | Select-Object DisplayName,DisplayVersion,InstallLocation,UninstallString,QuietUninstallString,EstimatedSize | ConvertTo-Json -Compress}else{'null'}",
    ],
    { encoding: 'utf8', windowsHide: true },
  ).trim();
}
const realRegistration = registration();
inputs.setupSha256 = await hash(executable);
inputs.appSha256 = await hash(expectedApp);
const sourcePath = resolve('installer/ui/setup.js');
inputs.setupUiSha256 = createHash('sha256')
  .update(await readFile(sourcePath))
  .digest('hex');
const expectedVersion = JSON.parse(
  await readFile(resolve('src-tauri/tauri.conf.json'), 'utf8'),
).version;

async function ownedRoot() {
  assert.deepEqual(JSON.parse(await readFile(ownerPath, 'utf8')), { nonce, pid: process.pid });
  assert.equal(await realpath(root), root);
}
async function freePort(port) {
  const reservation = createServer();
  await new Promise((done, reject) => {
    reservation.once('error', reject);
    reservation.listen(port, '127.0.0.1', done);
  });
  await new Promise((done) => reservation.close(done));
}
async function connect(directory) {
  await ownedRoot();
  inside(root, directory);
  const port = await setupDebugPort();
  // A fixed CI policy port must not attach to a different running setup.
  await until(
    async () => {
      try {
        await freePort(port);
        return true;
      } catch (error) {
        if (error.code === 'EADDRINUSE') return false;
        throw error;
      }
    },
    'owned setup CDP port becomes free',
    20_000,
  );
  const child = spawn(executable, [`/D=${directory}`], {
    env: {
      ...process.env,
      PRISM_SETUP_SANDBOX: root,
      WEBVIEW2_USER_DATA_FOLDER: join(root, 'SetupWebView'),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
    windowsHide: true,
    stdio: 'ignore',
  });
  active.add(child);
  child.on('error', (error) => {
    child.spawnError = error;
  });
  current = { child, browser: null, page: null, verified: false };
  await untilWebView(child, port, 'feature setup WebView', () => {
    if (child.spawnError) throw child.spawnError;
  });
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  current.browser = browser;
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => {
        try {
          const url = new URL(candidate.url());
          return (
            ['tauri:', 'http:', 'https:'].includes(url.protocol) &&
            (url.hostname === 'tauri.localhost' ||
              (url.protocol === 'tauri:' && url.hostname === 'localhost')) &&
            ['/', '/index.html'].includes(url.pathname) &&
            !url.searchParams.has('preview')
          );
        } catch {
          return false;
        }
      });
    return page;
  }, 'bundled feature setup page');
  current.page = page;
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
    else if (message.type() === 'warning') warnings.push(message.text());
  });
  current.invoke = async (command, args = {}) => {
    if (!current.verified)
      assert(
        ['setup_info', 'check_folder', 'scan_installation', 'read_log'].includes(command),
        'Unverified window cannot mutate setup.',
      );
    if (command === 'finish')
      assert.equal(args.launch, false, 'The bundled app must never launch.');
    return page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  };
  const info = await current.invoke('setup_info');
  (inputs.sessions ??= []).push({
    installDir: info.installDir,
    defaultDir: info.defaultDir,
    logPath: info.logPath,
    mode: info.mode,
    version: info.version,
  });
  assert.equal(pathKey(info.installDir), pathKey(directory));
  inside(root, info.installDir);
  inside(root, info.defaultDir);
  assert(info.logPath, 'This real native run must have its own log file.');
  inside(root, info.logPath);
  inside(root, await realpath(info.logPath));
  const logMetadata = await lstat(info.logPath);
  assert(logMetadata.isFile() && !logMetadata.isSymbolicLink());
  assert.equal(info.version, expectedVersion);
  assert.equal(info.ui, 'full');
  assert.equal(info.relaunch, false);
  assert.equal(info.payloadReady, true);
  await ownedRoot();
  // Read the actual embedded script through CDP, without bypassing CSP or replacing IPC.
  const cdp = await page.context().newCDPSession(page);
  const scripts = [];
  cdp.on('Debugger.scriptParsed', (script) => {
    if (/\/setup\.js(?:[?#]|$)/.test(script.url)) scripts.push(script);
  });
  await cdp.send('Debugger.enable');
  await until(() => scripts.length > 0, 'embedded setup script identity');
  const { scriptSource } = await cdp.send('Debugger.getScriptSource', {
    scriptId: scripts[0].scriptId,
  });
  assert.equal(
    createHash('sha256').update(scriptSource).digest('hex'),
    inputs.setupUiSha256,
    'Rebuild setup after the final UI/bridge guard change before this native test.',
  );
  await cdp.send('Debugger.disable');
  await cdp.detach();
  current.verified = true;
  current.info = info;
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'options');
  await expect(page.locator('#folder')).toHaveValue(info.installDir);
  await page.evaluate(async () => {
    window.__featureStages = [];
    window.__featureUnlisten = await window.__TAURI__.event.listen(
      'setup://progress',
      ({ payload }) => window.__featureStages.push(payload),
    );
  });
  return current;
}
async function capture(name) {
  assert(current?.verified, 'Only capture a verified private setup window.');
  await ownedRoot();
  await current.page.screenshot({ path: join(evidence, name) });
}
async function stop() {
  if (!current) return;
  assert(current.verified, 'Do not invoke mutations against an unverified window.');
  let closedReply;
  try {
    await current.invoke('finish', { launch: false });
  } catch (error) {
    closedReply = error;
  }
  const child = current.child;
  await until(() => child.exitCode !== null, 'owned feature setup exits', 90_000);
  assert.equal(child.exitCode, 0);
  if (closedReply)
    assert.match(
      String(closedReply),
      /Target.*closed|page.*closed|context.*closed|browser.*closed/i,
      'Only a native window closing before its IPC reply is expected.',
    );
  active.delete(child);
  await current.browser.close();
  current = null;
}
async function fileSnapshot(directory) {
  inside(root, directory);
  const record = JSON.parse(await readFile(join(directory, 'install.json'), 'utf8'));
  assert.equal(record.version, expectedVersion);
  const names = [...new Set([...record.files, 'uninstall.exe'])];
  const snapshot = {};
  for (const name of names) {
    const path = resolve(directory, name);
    inside(directory, path);
    const metadata = await lstat(path);
    assert(metadata.isFile() && !metadata.isSymbolicLink());
    snapshot[name] = await hash(path);
  }
  return snapshot;
}
async function doneSuccessfully() {
  await expect(current.page.locator('.setup')).toHaveAttribute('data-screen', 'done', {
    timeout: operationTimeout,
  });
  await expect(current.page.locator('#done-detail')).not.toHaveClass(/error/);
  await expect(current.page.locator('#track')).toHaveAttribute('aria-valuenow', '100');
  await current.page.locator('#launch').uncheck();
}
async function showLog() {
  await current.page.locator('.screen-done .detail-trigger').click();
  await expect(current.page.locator('#details')).toBeVisible();
  const log = await current.invoke('read_log');
  assert.equal(pathKey(log.path), pathKey(current.info.logPath));
  assert.equal(typeof log.truncated, 'boolean');
  await expect(current.page.locator('#log-text')).toHaveText(log.text);
  return log;
}

try {
  await connect(installA);
  let { page } = current;
  assert.equal(current.info.mode, 'install');
  await expect(page.locator('#install')).toBeDisabled();
  await page.locator('#language').selectOption('ko');
  await page.locator('#theme').selectOption('light');
  await capture('native-options-light.png');
  await page.locator('#theme').selectOption('dark');
  await capture('native-options-dark.png');
  await page.reload();
  await expect(page.locator('#language')).toHaveValue('ko');
  await expect(page.locator('#theme')).toHaveValue('dark');
  await expect(page.locator('#install')).toBeDisabled();
  // Reload replaced the read-only event listener; register it again on this verified window.
  await page.evaluate(async () => {
    window.__featureStages = [];
    await window.__TAURI__.event.listen('setup://progress', ({ payload }) =>
      window.__featureStages.push(payload),
    );
  });
  results.push('native language/theme selection persists across reload');
  await page.locator('#agree').check();
  await expect(page.locator('#install')).toBeEnabled();
  await page.locator('#install').click();
  await doneSuccessfully();
  const ownedNames = await verifyInstalledPayload(installA, expectedApp);
  const initialFiles = await fileSnapshot(installA);
  assert.equal(initialFiles['uninstall.exe'], inputs.setupSha256);
  const personal = join(installA, '개인 파일.txt');
  await writeFile(personal, 'Preserved native repair canary\n', { flag: 'wx' });
  const data = join(root, 'LocalAppData', 'app.prism.desktop', 'feature-canary.json');
  inside(root, data);
  await mkdir(resolve(data, '..'), { recursive: true });
  await writeFile(data, JSON.stringify({ nonce, preserve: true }), { flag: 'wx' });
  const originalData = await readFile(data);
  let log = await showLog();
  assert.match(log.text, /install: succeeded/);
  assert.match(log.text, /stage: Copying/);
  await capture('native-install-log.png');
  await page.keyboard.press('Escape');
  const firstStages = await page.evaluate(() => window.__featureStages);
  assert(firstStages.some((event) => event.stage === 'copying'));
  assert(firstStages.some((event) => event.stage === 'registering'));
  results.push('actual UI installation, all payload bytes, genuine stages and setup log');
  await stop();

  const notice = ownedNames.find((name) => name.startsWith('notices/'));
  assert(notice, 'Expected a real bundled component notice.');
  for (const name of ['prism.exe', notice]) {
    const path = resolve(installA, name);
    inside(installA, path);
    await unlink(path);
  }
  await connect(installA);
  page = current.page;
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'damaged');
  const checked = await current.invoke('check_folder', { path: installA });
  assert.equal(checked.health, 'damaged');
  assert.equal(checked.repairAvailable, true);
  assert.equal(checked.installedVersion, expectedVersion);
  await page.locator('#scan').click();
  const scan = await current.invoke('scan_installation', { installDir: installA });
  assert(scan.damaged.includes('prism.exe'));
  assert(scan.damaged.includes(notice));
  assert(scan.checked > 0 && scan.checked <= scan.total);
  assert.equal(scan.truncated, false);
  await expect(page.locator('#scan-detail')).toContainText(`${scan.checked}/${scan.total}`);
  await page.locator('#repair').click();
  // Each new process requires the original terms choice and the separate repair acknowledgement.
  await expect(page.locator('#terms')).toBeVisible();
  await page.locator('#terms-agree').click();
  await page.locator('#repair').click();
  await expect(page.locator('#repair-confirm')).toBeDisabled();
  await page.locator('#repair-ack').check();
  await page.locator('#repair-confirm').click();
  await doneSuccessfully();
  await verifyInstalledPayload(installA, expectedApp);
  assert.deepEqual(await fileSnapshot(installA), initialFiles);
  assert.equal(await readFile(personal, 'utf8'), 'Preserved native repair canary\n');
  assert.deepEqual(await readFile(data), originalData);
  log = await showLog();
  assert.match(log.text, /repair: succeeded/);
  await capture('native-repair-log.png');
  await page.keyboard.press('Escape');
  results.push(
    'actual missing app/notice scan and acknowledged repair restore every owned byte while retaining personal files/data',
  );
  const registrationA = await readFile(join(root, 'uninstall-entry.json'));
  await stop();

  await mkdir(installB);
  await connect(installB);
  page = current.page;
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'ready');
  await page.locator('#agree').check();
  await expect(page.locator('#install')).toBeEnabled();
  // After the genuine preflight, introduce an unrelated owned fixture file. The locked engine
  // must reject this changed folder without touching that file or the existing installation.
  const unknown = join(installB, 'unknown.txt');
  inside(root, unknown);
  await writeFile(unknown, 'Keep this unrelated file\n', { flag: 'wx' });
  await page.locator('#install').click();
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done', {
    timeout: operationTimeout,
  });
  await expect(page.locator('#done-detail')).toHaveClass(/error/);
  await expect(page.locator('#launch-option')).toBeHidden();
  assert.equal(await readFile(unknown, 'utf8'), 'Keep this unrelated file\n');
  assert.deepEqual(await fileSnapshot(installA), initialFiles);
  assert.equal(await readFile(personal, 'utf8'), 'Preserved native repair canary\n');
  assert.deepEqual(await readFile(data), originalData);
  assert.deepEqual(await readFile(join(root, 'uninstall-entry.json')), registrationA);
  log = await showLog();
  assert.match(log.text, /install: FAILED:/);
  await capture('native-failed-install-log.png');
  await page.keyboard.press('Escape');
  await capture('native-failed-install.png');
  await page.locator('#finish').click();
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'options');
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'conflict');
  await expect(page.locator('#install')).toBeDisabled();
  results.push(
    'genuine TOCTOU folder conflict: FAILED log, protected unrelated file/existing install and retry recheck',
  );
  await stop();
  assert.deepEqual(errors, []);
  assert.equal(registration(), realRegistration);
  assert.equal(await hash(executable), inputs.setupSha256);
  assert.equal(await hash(expectedApp), inputs.appSha256);
  await writeFile(
    join(evidence, 'result.json'),
    JSON.stringify({ inputs, results, errors, warnings, nonce, root }, null, 2),
  );
  console.log(JSON.stringify({ passed: results, errors, warnings, evidence }));
} catch (error) {
  if (current?.verified) await capture('failure.png').catch(() => {});
  await writeFile(
    join(evidence, 'result.json'),
    JSON.stringify(
      { inputs, results, errors, warnings, failure: String(error), nonce, root },
      null,
      2,
    ),
  );
  console.error(`Native setup feature evidence: ${evidence}`);
  throw error;
} finally {
  // Stop only processes this test spawned; retain the private fixture for diagnosis.
  for (const child of active) if (child.exitCode === null) child.kill();
  await current?.browser?.close().catch(() => {});
  assert.equal(
    registration(),
    realRegistration,
    'Real HKCU installation registration changed during private QA.',
  );
}
