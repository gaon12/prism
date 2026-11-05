// Real setup UI consumes a journal left by an actual process exit. All files, shortcuts
// and registration stay inside a marked OS-temp sandbox. No installed app is launched.
// Browser plugin not available: use repository Playwright/WebView2 CDP automation.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import { until, untilWebView } from './harness.mjs';
import { recoveryFixture } from './setup-fixture.mjs';

const executable = resolve(
  process.env.PRISM_SETUP_EXE ?? 'installer/src-tauri/target/debug/prism-setup.exe',
);
const fixture = recoveryFixture();
const root = await mkdtemp(join(tmpdir(), 'prism-recovery-test-'));
await writeFile(join(root, 'fixture-owner.txt'), 'Prism isolated recovery fixture');
const crash = spawnSync(
  fixture,
  ['--exact', 'recovery_tests::crash_fixture_process', '--ignored', '--nocapture'],
  {
    env: {
      ...process.env,
      PRISM_RECOVERY_TEST_ROOT: root,
      PRISM_RECOVERY_TEST_PHASE: 'registration',
      PRISM_RECOVERY_TEST_WAIT_FOR_KILL: '0',
    },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30000,
  },
);
assert.equal(crash.status, 73, crash.stderr);
const app = join(root, 'app');
assert.equal(await readFile(join(app, 'prism.exe'), 'utf8'), 'new');
assert((await readdir(app)).some((name) => name.startsWith('.prism-setup-')));
await writeFile(
  join(app, 'install.json'),
  'damaged current record; original is in the checked backup',
);
const before = await readFile(join(root, 'uninstall-entry.json'), 'utf8');
const profile = join(root, 'AppProfile');
await mkdir(profile);
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    scopes: [],
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    startOnLogin: false,
  }),
);
const server = createServer();
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const port = server.address().port;
await new Promise((done) => server.close(done));
const child = spawn(executable, [`/D=${app}`], {
  env: {
    ...process.env,
    PRISM_SETUP_SANDBOX: root,
    PRISM_DEV_DATA_DIR: profile,
    WEBVIEW2_USER_DATA_FOLDER: join(root, 'SetupWebView'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
  windowsHide: true,
  stdio: 'ignore',
});
let spawnError;
child.on('error', (error) => {
  spawnError = error;
});
let browser;
const errors = [],
  warnings = [];
try {
  await untilWebView(child, port, 'recovery setup endpoint', () => {
    if (spawnError) throw spawnError;
  });
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page;
  await until(() => {
    page = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((page) => page.url().includes('tauri.localhost'));
    return page;
  }, 'bundled recovery setup page');
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
    if (message.type() === 'warning') warnings.push(message.text());
  });
  assert.equal(await page.title(), 'Prism Setup');
  await page.locator('.setup[data-screen="options"]').waitFor();
  await until(
    () => page.locator('#install').isEnabled(),
    'damaged record still allows checked recovery',
  );
  const pendingMessage = await page.evaluate(() => {
    const index = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'].indexOf(document.documentElement.lang);
    return window.SETUP_MESSAGES.recoveryPending[index];
  });
  await page.locator('#folder-hint').filter({ hasText: pendingMessage }).waitFor();
  assert(
    !(await page.locator('#version').innerText()).includes('0.2.0'),
    'uncommitted metadata is not presented as a completed installed version',
  );
  const evidence = join(tmpdir(), 'prism-ui-evidence');
  await mkdir(evidence, { recursive: true });
  await page.screenshot({ path: join(evidence, 'setup-recovery-options-native.png') });
  // Displaying the setup/space estimate must not silently restore application files.
  assert.equal(await readFile(join(app, 'prism.exe'), 'utf8'), 'new');
  assert.equal(await readFile(join(root, 'uninstall-entry.json'), 'utf8'), before);
  await page.evaluate(async () => {
    window.__recoveryEvents = [];
    window.__recoveryLabels = [];
    window.__recoveryObserver = new MutationObserver((records) => {
      for (const record of records)
        for (const node of record.addedNodes) {
          if (node.nodeType === Node.TEXT_NODE) window.__recoveryLabels.push(node.textContent);
        }
    });
    window.__recoveryObserver.observe(document.getElementById('stage'), { childList: true });
    window.__recoveryUnlisten = await window.__TAURI__.event.listen(
      'setup://progress',
      ({ payload }) => {
        window.__recoveryEvents.push({
          stage: payload.stage,
          label: document.getElementById('stage').textContent,
        });
      },
    );
  });
  await page.locator('#install').click();
  await page.locator('.setup[data-screen="done"]').waitFor();
  assert.equal(
    await page.locator('#done-detail').evaluate((node) => node.classList.contains('error')),
    false,
  );
  const events = await page.evaluate(() => window.__recoveryEvents);
  const recovery = events.find((event) => event.stage === 'recovering');
  assert(recovery, 'actual recovery stage must be emitted before the new copy');
  const labels = await page.evaluate(() => window.__recoveryLabels);
  const recoveryLabels = [
    '중단된 설치 복구 중',
    'Recovering interrupted setup',
    '正在恢复中断的安装',
    '正在復原中斷的安裝',
    '中断したインストールを復旧中',
  ];
  // Independent event listeners may run before or after the UI listener. Observe
  // actual DOM writes instead of assuming event delivery implies synchronous paint.
  const renderedRecovery = labels.find((label) => recoveryLabels.includes(label));
  assert(renderedRecovery, 'localized recovery stage must be rendered');
  const expected = resolve(
    process.env.PRISM_SETUP_EXPECTED_EXE ?? 'src-tauri/target/debug/prism.exe',
  );
  const hash = async (path) =>
    createHash('sha256')
      .update(await readFile(path))
      .digest('hex');
  assert.equal(await hash(join(app, 'prism.exe')), await hash(expected));
  assert.equal(await readFile(join(app, 'personal.txt'), 'utf8'), 'user file');
  assert(!(await readdir(app)).some((name) => name.startsWith('.prism-setup-')));
  const record = JSON.parse(await readFile(join(app, 'install.json'), 'utf8'));
  assert.equal(record.version, JSON.parse(await readFile('package.json', 'utf8')).version);
  assert(record.files.includes('prism.exe'));
  await page.screenshot({ path: join(evidence, 'setup-recovery-done-native.png') });
  await page.locator('#launch').uncheck();
  await page.locator('#finish').click();
  await until(() => child.exitCode !== null, 'owned recovery setup exit');
  assert.equal(child.exitCode, 0);
  assert.deepEqual(errors, []);
  assert.deepEqual(warnings, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      actualCrashExit: 73,
      recoveredStage: renderedRecovery,
      userFilePreserved: true,
      appLaunches: 0,
      registration: 'sandbox JSON',
    }),
  );
} finally {
  await browser?.close();
  if (child.exitCode === null) child.kill();
}
