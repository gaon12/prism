// Real installed uninstaller -> checkbox choice -> actual data retention/removal.
// Browser plugin not available: use the repository Playwright/WebView2 harness.
// Every writable folder is a fresh OS-temp sandbox; no installed app is launched.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, relative, resolve, isAbsolute } from 'node:path';
import { chromium } from '@playwright/test';
import { setupDebugPort, until } from './harness.mjs';
import { verifyInstalledPayload } from './setup-payload.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_SETUP_EXE, 'Supply the candidate setup explicitly.');
assert(process.env.PRISM_SETUP_EXPECTED_EXE, 'Supply the actual bundled app explicitly.');
const executable = resolve(process.env.PRISM_SETUP_EXE);
const expectedApp = resolve(process.env.PRISM_SETUP_EXPECTED_EXE);
const root = await mkdtemp(join(tmpdir(), 'prism-setup-data-choice-'));
await writeFile(join(root, 'fixture-owner.txt'), 'Prism Q61 native fixture\n');
const exists = (path) =>
  stat(path).then(
    () => true,
    () => false,
  );
async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
function inside(parent, path) {
  const suffix = relative(parent, path);
  assert(suffix && suffix !== '..' && !suffix.startsWith('..\\') && !isAbsolute(suffix));
}
function systemSnapshot() {
  // Only the known registration and named service are inspected. No mutation.
  return JSON.parse(
    execFileSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        "$ErrorActionPreference='Stop'; $key='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prism'; $entry=if(Test-Path -LiteralPath $key){Get-ItemProperty -LiteralPath $key | Select-Object DisplayName,DisplayVersion,InstallLocation,UninstallString,QuietUninstallString,EstimatedSize}else{$null}; $service=Get-CimInstance Win32_Service -Filter \"Name='PrismFileIndex'\" | Select-Object Name,State,ProcessId,PathName; @{registration=$entry;service=$service}|ConvertTo-Json -Depth 4 -Compress",
      ],
      { encoding: 'utf8', windowsHide: true },
    ).trim(),
  );
}
const systemBefore = systemSnapshot();
const setupSha256 = await digest(executable);
const appSha256 = await digest(expectedApp);
const errors = [],
  warnings = [],
  results = [];
const active = new Set();
let browser;
function start(path, args, environment) {
  const child = spawn(path, args, { env: environment, windowsHide: true, stdio: 'ignore' });
  child.on('error', (error) => {
    child.spawnError = error;
  });
  active.add(child);
  return child;
}
async function exited(child) {
  await until(
    () => {
      if (child.spawnError) throw child.spawnError;
      return child.exitCode !== null;
    },
    'owned setup exit',
    90_000,
  );
  assert.equal(child.exitCode, 0);
  active.delete(child);
}
try {
  for (const removeData of [false, true]) {
    const sandbox = join(root, removeData ? 'delete' : 'keep');
    const installed = join(sandbox, '설치 폴더');
    inside(root, sandbox);
    inside(sandbox, installed);
    await mkdir(sandbox);
    const environment = { ...process.env, PRISM_SETUP_SANDBOX: sandbox };
    // Node spawns the executable directly; /S cannot be rewritten by Git Bash.
    await exited(start(executable, ['/S', `/D=${installed}`], environment));
    await verifyInstalledPayload(installed, expectedApp);
    assert.equal(await digest(join(installed, 'uninstall.exe')), setupSha256);

    const seeded = [];
    for (const [folder, name] of [
      ['RoamingAppData', 'settings.json'],
      ['RoamingAppData', 'session.json'],
      ['LocalAppData', 'clipboard/history.json'],
      ['LocalAppData', 'clipboard/images/fixture.png'],
      ['LocalAppData', 'content/index.sqlite'],
      ['LocalAppData', 'models/fixture.gguf'],
      ['LocalAppData', 'collections/thumbnail.webp'],
    ]) {
      const path = join(sandbox, folder, 'app.prism.desktop', name);
      inside(sandbox, path);
      const bytes = Buffer.from(`Q61 private fixture: ${folder}/${name}\n`);
      await mkdir(resolve(path, '..'), { recursive: true });
      await writeFile(path, bytes);
      seeded.push({ path, bytes });
    }
    // Adjacent applications and a user's file inside the install folder survive
    // either choice, even when Prism's own cache trees are removed recursively.
    const canaries = [
      join(root, 'outside-sandbox.txt'),
      join(installed, '사용자 문서.txt'),
      join(sandbox, 'LocalAppData', 'app.prism.desktop-other', 'keep.txt'),
      join(sandbox, 'RoamingAppData', 'OtherApplication', 'keep.txt'),
    ];
    for (const path of canaries) {
      inside(root, path);
      await mkdir(resolve(path, '..'), { recursive: true });
      await writeFile(path, 'preserved private canary\n');
    }
    const port = await setupDebugPort();
    const child = start(join(installed, 'uninstall.exe'), [], {
      ...environment,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    });
    await until(
      async () => {
        if (child.spawnError) throw child.spawnError;
        // The bootstrap normally exits before its owned temporary UI opens.
        if (child.exitCode !== null) assert.equal(child.exitCode, 0);
        try {
          return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
        } catch {
          return false;
        }
      },
      'temporary uninstaller WebView',
      90_000,
    );
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    let page;
    await until(() => {
      page = browser
        .contexts()
        .flatMap((context) => context.pages())
        .find((page) => page.url().includes('tauri.localhost'));
      return page;
    }, 'uninstall page');
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text());
      if (message.type() === 'warning') warnings.push(message.text());
    });
    assert.equal(await page.title(), 'Prism Setup');
    await page.locator('.setup[data-screen="uninstall"]').waitFor();
    assert.equal(await page.locator('vite-error-overlay').count(), 0);
    assert.equal(await page.locator('#uninstall-folder').innerText(), installed);
    const checkbox = page.locator('#remove-data');
    assert.equal(await checkbox.isChecked(), false, 'Data is retained by default.');
    if (removeData) await checkbox.check();
    assert.equal(await checkbox.isChecked(), removeData);
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      true,
    );
    const optionScreenshot = join(root, `${removeData ? 'delete' : 'keep'}-option.png`);
    await page.screenshot({ path: optionScreenshot });
    // Prove the UI has not already deleted anything before explicit submission.
    for (const { path, bytes } of seeded) assert.deepEqual(await readFile(path), bytes);
    await page.locator('#uninstall-form button[type="submit"]').click();
    await page.locator('.setup[data-screen="done"]').waitFor({ timeout: 90_000 });
    assert.equal(
      await page.locator('#done-detail').evaluate((node) => node.classList.contains('error')),
      false,
    );
    const expectedMessage = await page.evaluate((removeData) => {
      const index = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'].indexOf(document.documentElement.lang);
      return window.SETUP_MESSAGES[removeData ? 'removedData' : 'keptData'][index];
    }, removeData);
    assert.equal(await page.locator('#done-detail').innerText(), expectedMessage);
    for (const { path, bytes } of seeded) {
      if (removeData) assert.equal(await exists(path), false);
      else assert.deepEqual(await readFile(path), bytes);
    }
    for (const folder of ['LocalAppData', 'RoamingAppData'])
      assert.equal(await exists(join(sandbox, folder, 'app.prism.desktop')), !removeData);
    for (const path of canaries)
      assert.equal(await readFile(path, 'utf8'), 'preserved private canary\n');
    for (const name of ['prism.exe', 'uninstall.exe', 'install.json'])
      assert.equal(await exists(join(installed, name)), false);
    assert.equal(await exists(join(sandbox, 'uninstall-entry.json')), false);
    const doneScreenshot = join(root, `${removeData ? 'delete' : 'keep'}-done.png`);
    await page.screenshot({ path: doneScreenshot });
    await page.locator('#finish').click();
    await browser.close();
    browser = undefined;
    await exited(child);
    await until(
      async () =>
        !(await readdir(tmpdir())).some((name) => name.startsWith(`prism-uninstall-${child.pid}-`)),
      'owned temporary copy cleanup',
    );
    assert.deepEqual(
      systemSnapshot(),
      systemBefore,
      'Real registration and service are unchanged.',
    );
    results.push({
      removeData,
      dataFixtures: seeded.length,
      canaries: canaries.length,
      optionScreenshot,
      doneScreenshot,
      passed: true,
    });
  }
  assert.equal(await digest(executable), setupSha256);
  assert.equal(await digest(expectedApp), appSha256);
  assert.deepEqual(errors, []);
  assert.deepEqual(warnings, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      setupSha256,
      appSha256,
      results,
      realRegistrationAndServiceUnchanged: true,
      appLaunched: false,
      errors,
      warnings,
    }),
  );
} finally {
  // Close only our test window. Never find or kill other Prism installations.
  if (browser?.isConnected()) {
    const page = browser.contexts().flatMap((context) => context.pages())[0];
    await page
      ?.evaluate(() => window.__TAURI_INTERNALS__.invoke('finish', { launch: false }))
      .catch(() => {});
    await browser.close();
  }
  for (const child of active) if (child.exitCode === null) child.kill();
}
