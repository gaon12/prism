// Browser plugin not available: repository Playwright + actual bundled setup WebView2.
// Version labels and old app bytes are synthetic. No app binary is launched.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import { until, untilWebView } from './harness.mjs';
import { recoveryFixture } from './setup-fixture.mjs';

const executable = resolve(
  process.env.PRISM_SETUP_EXE ?? 'installer/src-tauri/target/debug/prism-setup.exe',
);
const root = await mkdtemp(join(tmpdir(), 'prism-recovery-test-'));
await writeFile(join(root, 'fixture-owner.txt'), 'Prism isolated recovery fixture');
await copyFile(executable, join(root, 'fixture-setup.exe'));
const fixture = spawnSync(
  recoveryFixture(),
  ['--exact', 'recovery_tests::crash_fixture_process', '--ignored', '--nocapture'],
  {
    env: {
      ...process.env,
      PRISM_RECOVERY_TEST_ROOT: root,
      PRISM_RECOVERY_TEST_PHASE: 'previous-ready',
      PRISM_RECOVERY_TEST_WAIT_FOR_KILL: '0',
    },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30000,
  },
);
assert.equal(fixture.status, 0, fixture.stderr);
const app = join(root, 'app');
const environment = { ...process.env, PRISM_SETUP_SANDBOX: root };
const record = async () => JSON.parse(await readFile(join(app, 'install.json'), 'utf8'));
const hash = async (path) =>
  createHash('sha256')
    .update(await readFile(path))
    .digest('hex');
const oldSetupHash = await hash(join(app, 'uninstall.exe'));
const version = JSON.parse(await readFile('package.json', 'utf8')).version;
const runSilent = (...args) => {
  const result = spawnSync(executable, ['/S', `/D=${app}`, ...args], {
    env: environment,
    windowsHide: true,
    encoding: 'utf8',
    timeout: 180000,
  });
  assert.equal(result.status, 0, result.stderr);
};
runSilent();
assert.equal((await record()).version, version);
assert((await readdir(app)).some((name) => name.startsWith('.prism-previous-')));
const currentHash = await hash(join(app, 'prism.exe'));
assert.equal(
  currentHash,
  await hash(resolve(process.env.PRISM_SETUP_EXPECTED_EXE ?? 'src-tauri/target/debug/prism.exe')),
);
const missingVersion = spawnSync(executable, ['/S', '--restore-version', `/D=${app}`], {
  env: environment,
  windowsHide: true,
  encoding: 'utf8',
  timeout: 10000,
});
assert.equal(missingVersion.status, 1, 'missing restore target must fail silently');
assert.equal(await hash(join(app, 'prism.exe')), currentHash);
// The test's data sentinel is outside the app folder and must never be rolled back.
const userData = join(root, 'AppData', 'user-document.md');
await mkdir(join(root, 'AppData'), { recursive: true });
await writeFile(userData, 'newer user data');
const server = createServer();
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const port = server.address().port;
await new Promise((done) => server.close(done));
const child = spawn(executable, [`/D=${app}`], {
  env: {
    ...environment,
    WEBVIEW2_USER_DATA_FOLDER: join(root, 'SetupWebView'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
  windowsHide: true,
  stdio: 'ignore',
});
let browser;
const errors = [],
  warnings = [];
try {
  await untilWebView(child, port, 'previous-version setup endpoint');
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((page) => page.url().includes('tauri.localhost'));
    return page;
  }, 'actual previous-version setup page');
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
    if (message.type() === 'warning') warnings.push(message.text());
  });
  assert.equal(await page.title(), 'Prism Setup');
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  await page.locator('.setup[data-screen="options"]').waitFor();
  await page.locator('#previous-options').waitFor();
  const info = await invoke('setup_info');
  assert.equal(info.previousVersion.version, '0.0.1');
  assert.equal(
    await hash(join(app, 'prism.exe')),
    currentHash,
    'read-only inspection does not restore',
  );
  // A stale target is rejected without changing the installation.
  await assert.rejects(invoke('run_restore', { installDir: app, expectedVersion: '0.0.0' }));
  assert.equal(await hash(join(app, 'prism.exe')), currentHash);
  await page.locator('#previous-options').click();
  await page.locator('.setup[data-screen="restore"]').waitFor();
  assert.equal(await page.locator('#restore').isDisabled(), true);
  assert((await page.locator('#restore-title').innerText()).includes('0.0.1'));
  const evidence = join(tmpdir(), 'prism-ui-evidence');
  await mkdir(evidence, { recursive: true });
  await page.screenshot({ path: join(evidence, 'setup-previous-confirm-native.png') });
  await page.locator('#restore-back').click();
  await page.locator('.setup[data-screen="options"]').waitFor();
  assert.equal(await hash(join(app, 'prism.exe')), currentHash, 'back is read-only');
  await page.locator('#previous-options').click();
  await page.locator('#restore-ack').check();
  await page.locator('#restore').click();
  await page.locator('.setup[data-screen="done"]').waitFor();
  assert.equal(
    await page.locator('#done-detail').evaluate((node) => node.classList.contains('error')),
    false,
  );
  assert.equal(await readFile(join(app, 'prism.exe'), 'utf8'), 'old');
  assert.equal((await record()).version, '0.0.1');
  assert.equal(await hash(join(app, 'uninstall.exe')), oldSetupHash);
  assert.equal(await readFile(join(app, 'personal.txt'), 'utf8'), 'user file');
  assert.equal(await readFile(userData, 'utf8'), 'newer user data');
  assert.equal(
    (await readdir(app)).some((name) => name.startsWith('.prism-previous-')),
    false,
  );
  assert.equal(
    await page.locator('#launch').isChecked(),
    false,
    'restored app does not launch automatically',
  );
  await page.screenshot({ path: join(evidence, 'setup-previous-done-native.png') });
  await page.locator('#finish').click();
  await until(() => child.exitCode !== null, 'owned previous setup exit');
  assert.equal(child.exitCode, 0);
  await browser.close();
  browser = null;
  // Update once more, then restore through the installed uninstaller's temp copy.
  runSilent();
  assert.equal((await record()).version, version);
  const copiesBefore = new Set(
    (await readdir(tmpdir())).filter((name) => /^prism-uninstall-\d+-\d+$/.test(name)),
  );
  const bootstrap = spawnSync(join(app, 'uninstall.exe'), ['/S', '--restore-version', '0.0.1'], {
    env: environment,
    windowsHide: true,
    timeout: 30000,
  });
  assert.equal(bootstrap.status, 0);
  await until(
    async () => {
      try {
        return (await record()).version === '0.0.1';
      } catch {
        return false;
      }
    },
    'silent restore through temp copy',
    60000,
  );
  await until(
    async () =>
      !(await readdir(tmpdir())).some(
        (name) => /^prism-uninstall-\d+-\d+$/.test(name) && !copiesBefore.has(name),
      ),
    'owned restore temp copy cleaned',
  );
  assert.equal(await readFile(join(app, 'prism.exe'), 'utf8'), 'old');
  assert.equal(await readFile(userData, 'utf8'), 'newer user data');
  assert.deepEqual(errors, []);
  assert.deepEqual(warnings, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      guiRestore: true,
      silentTempRestore: true,
      userFilesPreserved: true,
      appLaunches: 0,
      registration: 'sandbox JSON',
      versions: 'synthetic labels and inert old app',
    }),
  );
} finally {
  await browser?.close();
  if (child.exitCode === null) child.kill();
}
