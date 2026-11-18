// Browser plugin not available: exercise the real setup HTML/CSS/JS in Playwright.
// Preview cases are labeled fixtures. IPC cases validate browser behavior and command
// contracts, not native filesystem writes, file integrity, registration or installation.
import { chromium, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { join, extname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
const uiRoot = fileURLToPath(new URL('../../installer/ui/', import.meta.url));
const evidence = await mkdtemp(join(tmpdir(), 'prism-setup-ui-'));
const assets = new Set(['index.html', 'setup.js', 'setup.css', 'messages.js', 'terms.js']);
const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
    if (pathname === '/favicon.ico') {
      response.writeHead(204).end();
      return;
    }
    const name = pathname === '/' ? 'index.html' : pathname.slice(1);
    if (!assets.has(name)) {
      response.writeHead(404).end();
      return;
    }
    response.setHeader(
      'Content-Type',
      { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' }[extname(name)] ||
        'text/html',
    );
    response.end(await readFile(join(uiRoot, name)));
  } catch {
    response.writeHead(500).end();
  }
});
await new Promise((resolve, reject) => {
  server.once('error', reject);
  server.listen(0, '127.0.0.1', resolve);
});
const baseUrl = `http://127.0.0.1:${server.address().port}`;
let browser;
let activePage;
const checks = [],
  errors = [];
function observe(page) {
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
}
async function checkBounds(page, label) {
  const bounds = await page.evaluate(() => {
    const workspace = document.querySelector('.workspace');
    const modal = [...document.querySelectorAll('.modal')].find((element) => !element.hidden);
    return {
      window: document.documentElement.scrollWidth <= innerWidth,
      workspace: workspace.scrollHeight <= workspace.clientHeight + 1,
      modal: !modal || modal.getBoundingClientRect().bottom <= innerHeight,
    };
  });
  assert.deepEqual(bounds, { window: true, workspace: true, modal: true }, label);
  checks.push(label);
}
try {
  browser = await chromium.launch();
  const previewContext = await browser.newContext();
  const previewPage = await previewContext.newPage();
  activePage = previewPage;
  observe(previewPage);
  for (const width of [960, 760]) {
    await previewPage.setViewportSize({ width, height: width === 960 ? 640 : 520 });
    for (const language of ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'])
      for (const theme of ['light', 'dark']) {
        const tag = `${width}-${language}-${theme}`;
        await previewPage.goto(`${baseUrl}/?preview=options&lang=${language}&theme=${theme}`);
        await expect(previewPage.locator('#location-state')).toHaveAttribute('data-state', 'ready');
        await expect(previewPage).toHaveTitle('Prism Setup');
        await expect(previewPage.locator('#install')).toBeDisabled();
        await expect(previewPage.locator('#close-running-install')).not.toBeChecked();
        await checkBounds(previewPage, `${tag}-options`);
        await previewPage.check('#agree');
        await expect(previewPage.locator('#install')).toBeEnabled();
        if (width === 960 && language === 'ko')
          await previewPage.screenshot({ path: join(evidence, `options-${theme}.png`) });
        await previewPage.click('#open-terms');
        await checkBounds(previewPage, `${tag}-terms`);
        await expect(previewPage.locator('#terms-body h3')).toHaveCount(5);
        for (let count = 0; count < 5; count++) await previewPage.keyboard.press('Tab');
        assert.equal(
          await previewPage.evaluate(() =>
            document.querySelector('#terms').contains(document.activeElement),
          ),
          true,
        );
        await previewPage.keyboard.press('Escape');
        await previewPage.click('.screen-options .detail-trigger');
        await expect(previewPage.locator('#log-text')).toContainText('Preview fixture');
        await checkBounds(previewPage, `${tag}-details`);
        await previewPage.keyboard.press('Escape');
        for (const screen of [
          'installed',
          'damaged',
          'recovery',
          'conflict',
          'error',
          'uninstall',
          'restore',
          'progress',
        ]) {
          await previewPage.goto(`${baseUrl}/?preview=${screen}&lang=${language}&theme=${theme}`);
          const expected =
            screen === 'error'
              ? 'done'
              : screen === 'progress'
                ? 'progress'
                : screen === 'uninstall'
                  ? 'uninstall'
                  : screen === 'restore'
                    ? 'restore'
                    : 'options';
          await expect(previewPage.locator('.setup')).toHaveAttribute('data-screen', expected);
          await checkBounds(previewPage, `${tag}-${screen}`);
          if (screen === 'damaged') {
            await previewPage.click('#scan');
            await expect(previewPage.locator('#scan-detail')).toBeVisible();
            await checkBounds(previewPage, `${tag}-partial-scan`);
          }
          if (screen === 'conflict') await expect(previewPage.locator('#install')).toBeDisabled();
          if (screen === 'progress') await expect(previewPage.locator('#close')).toBeDisabled();
          if (screen === 'restore') {
            await expect(previewPage.locator('#close-running-restore')).not.toBeChecked();
            await expect(previewPage.locator('#restore')).toBeDisabled();
            await previewPage.check('#restore-ack');
            await expect(previewPage.locator('#restore')).toBeEnabled();
          }
          if (screen === 'uninstall') {
            await expect(previewPage.locator('#remove-data')).not.toBeChecked();
            await expect(previewPage.locator('#close-running-uninstall')).not.toBeChecked();
          }
        }
      }
  }
  await previewPage.goto(`${baseUrl}/?preview=options`);
  await previewPage.selectOption('#language', 'ja');
  await previewPage.selectOption('#theme', 'system');
  await previewPage.emulateMedia({ colorScheme: 'dark' });
  await expect(previewPage.locator('html')).toHaveAttribute('data-theme', 'dark');
  await previewPage.emulateMedia({ colorScheme: 'light' });
  await expect(previewPage.locator('html')).toHaveAttribute('data-theme', 'light');
  await previewPage.selectOption('#theme', 'dark');
  await previewPage.reload();
  await expect(previewPage.locator('#theme')).toHaveValue('dark');
  await expect(previewPage.locator('#language')).toHaveValue('ja');
  checks.push('system appearance follows OS changes; language and theme survive reload');
  await previewContext.close();
  const missingContext = await browser.newContext({ locale: 'en-US' });
  await missingContext.route(/^https?:\/\/tauri\.localhost\//, async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    const name = pathname === '/' ? 'index.html' : pathname.slice(1);
    if (!assets.has(name)) {
      await route.fulfill({ status: 404, body: '' });
      return;
    }
    await route.fulfill({
      contentType: { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' }[
        extname(name)
      ],
      body: await readFile(join(uiRoot, name)),
    });
  });
  for (const [url, malformed] of [
    [baseUrl, false],
    [`${baseUrl}/?preview=unknown`, false],
    ['http://tauri.localhost/?preview=progress', false],
    ['https://tauri.localhost/?preview=options', false],
    [`${baseUrl}/?preview=options`, true],
  ]) {
    const missingPage = await missingContext.newPage();
    activePage = missingPage;
    observe(missingPage);
    if (malformed)
      await missingPage.addInitScript(() => {
        window.__TAURI__ = {};
      });
    await missingPage.goto(url);
    await expect(missingPage.locator('.setup')).toHaveAttribute('data-screen', 'done');
    await expect(missingPage.locator('.setup')).toHaveClass(/failed/);
    await missingPage.selectOption('#language', 'en');
    await expect(missingPage.locator('#done-detail')).toContainText(
      'Could not connect to the setup components',
    );
    await expect(missingPage.locator('#install')).toBeDisabled();
    await expect(missingPage.locator('#finish')).toBeDisabled();
    await expect(missingPage.locator('#version')).toBeEmpty();
    assert.equal(
      await missingPage.locator('button:not(:disabled),input:not(:disabled)').count(),
      0,
    );
    await missingPage.selectOption('#language', 'ko');
    await expect(missingPage.locator('#done-detail')).toContainText(
      '설치 구성 요소에 연결하지 못했습니다',
    );
    await missingPage.keyboard.press('Escape');
    checks.push(`missing native bridge fails closed: ${url} malformed=${malformed}`);
    await missingPage.close();
  }
  await missingContext.close();
  const context = await browser.newContext({
    locale: 'en-US',
    viewport: { width: 960, height: 640 },
    permissions: ['clipboard-read', 'clipboard-write'],
  });
  const page = await context.newPage();
  activePage = page;
  observe(page);
  await page.addInitScript(() => {
    const dir = 'C:\\prism-ui-contract';
    let progress = () => {},
      completeRepair;
    let installs = 0;
    const f = {
      directory: dir,
      calls: [],
      pending: [],
      logFailure: false,
      healthOverride: null,
      previousVersion: null,
      choosePath: null,
      suppressRepairProgress: false,
      mode: sessionStorage.getItem('prism.fixture.mode') || 'install',
      repairError: null,
      installError: null,
      uninstallError: null,
      restoreError: null,
      skipInstallFailure: false,
      emitProgress: (payload) => progress(payload),
      completeRepair: () => completeRepair(),
      release: (i, error, previousVersion = null) =>
        f.pending[i]({
          error,
          health: error ? 'conflict' : 'empty',
          freeBytes: 100000000,
          requiredBytes: 1000,
          repairAvailable: false,
          recoveryPending: false,
          previousVersion,
        }),
    };
    window.__fixture = f;
    window.__TAURI__ = {
      core: {
        async invoke(command, args = {}) {
          f.calls.push({ command, args });
          switch (command) {
            case 'setup_info':
              return {
                mode: f.mode,
                ui: 'full',
                version: '0.1.0',
                installedVersion: '0.1.0',
                installDir: dir,
                defaultDir: dir,
                desktopShortcut: true,
                startMenuShortcut: true,
                requiredBytes: 1000,
                freeBytes: 100000000,
                payloadReady: true,
                relaunch: false,
                repairAvailable: true,
                previousVersion:
                  f.mode === 'restore'
                    ? { currentVersion: '0.1.0', version: '0.0.9', requiredBytes: 1000 }
                    : null,
              };
            case 'check_folder':
              if (args.path.endsWith('slow')) return new Promise((r) => f.pending.push(r));
              return {
                error: args.path.endsWith('conflict') ? 'Fixture folder conflict' : null,
                health: args.path.endsWith('conflict')
                  ? 'conflict'
                  : f.healthOverride || 'installed',
                freeBytes: 100000000,
                requiredBytes: 1000,
                repairAvailable: args.path === dir && f.healthOverride !== 'empty',
                recoveryPending: false,
                previousVersion: f.previousVersion,
              };
            case 'choose_folder':
              return f.choosePath;
            case 'scan_installation':
              return { damaged: ['prism.exe'], checked: 3, total: 10, truncated: true };
            case 'run_repair':
              if (f.repairError) throw f.repairError;
              if (!f.suppressRepairProgress)
                progress({
                  stage: args.closeRunningApp ? 'closing' : 'copying',
                  fraction: args.closeRunningApp ? 0 : 0.4,
                });
              return new Promise((r) => {
                completeRepair = () => {
                  progress({ stage: 'copying', fraction: 0.4 });
                  r();
                };
              });
            case 'run_install':
              if (f.installError) throw f.installError;
              progress({ stage: 'copying', fraction: 0.6 });
              if (++installs === 1 && !f.skipInstallFailure) throw 'Fixture install failure';
              return;
            case 'run_uninstall':
              if (f.uninstallError) throw f.uninstallError;
              progress({ stage: 'removing', fraction: 0.6 });
              return;
            case 'run_restore':
              if (f.restoreError) throw f.restoreError;
              progress({ stage: 'copying', fraction: 0.6 });
              return;
            case 'read_log':
              if (f.logFailure) throw 'Fixture log permission denied';
              return {
                path: 'C:\\fixture\\setup.log',
                text: 'Contract fixture: setup event received\n',
                truncated: true,
              };
            case 'finish':
            case 'open_log_folder':
              return;
            default:
              throw 'Unexpected fixture command ' + command;
          }
        },
      },
      event: {
        async listen(name, handler) {
          progress = (payload) => handler({ payload });
          return () => {};
        },
      },
      window: { getCurrentWindow: () => ({ minimize: async () => {}, close: async () => {} }) },
    };
  });
  await page.goto(baseUrl);
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'installed');
  await expect(page.locator('#install')).toBeDisabled();
  await page.click('#repair');
  await expect(page.locator('#terms')).toBeVisible();
  assert.equal(
    await page.evaluate(() => window.__fixture.calls.some((call) => call.command === 'run_repair')),
    false,
  );
  await page.click('#terms-agree');
  await expect(page.locator('#agree')).toBeChecked();
  await page.click('#scan');
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'damaged');
  await expect(page.locator('#scan-detail')).toContainText('3 of 10 files checked');
  await expect(page.locator('#scan-detail')).toContainText('results are partial');
  checks.push('scan actual command and bounded result display');
  await page.click('#repair');
  await expect(page.locator('#repair-confirm')).toBeDisabled();
  await expect(page.locator('#close-running-repair')).not.toBeChecked();
  await page.check('#repair-ack');
  await page.click('#repair-cancel');
  await page.click('#repair');
  await expect(page.locator('#repair-ack')).not.toBeChecked();
  await expect(page.locator('#repair-confirm')).toBeDisabled();
  await page.check('#repair-ack');
  await page.click('#repair-confirm');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'progress');
  await expect(page.locator('#close')).toBeDisabled();
  assert.deepEqual(
    await page.evaluate(() => window.__fixture.calls.find((x) => x.command === 'run_repair').args),
    { installDir: 'C:\\prism-ui-contract', closeRunningApp: false },
  );
  await page.click('.screen-progress .detail-trigger');
  await expect(page.locator('#log-text')).toContainText('Contract fixture');
  await expect(page.locator('#log-scope')).toContainText('Only the last part');
  await page.click('#copy-log');
  assert.equal(
    (await page.evaluate(() => navigator.clipboard.readText())).replaceAll('\r\n', '\n'),
    'Contract fixture: setup event received\n',
  );
  await expect(page.locator('#log-scope')).toBeVisible();
  await page.evaluate(() => {
    window.__fixture.logFailure = true;
  });
  await page.click('#refresh-log');
  await expect(page.locator('#log-notice')).toContainText('Fixture log permission denied');
  await expect(page.locator('#log-notice')).toHaveClass(/error/);
  await page.evaluate(() => {
    window.__fixture.logFailure = false;
  });
  await page.click('#refresh-log');
  await expect(page.locator('#log-notice')).toBeEmpty();
  await page.keyboard.press('Escape');
  await page.evaluate(() => window.__fixture.completeRepair());
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  await expect(page.locator('#launch')).not.toBeChecked();
  checks.push('repair explicit target, acknowledgement, progress, log API and copy');
  await page.reload();
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'installed');
  await page.check('#agree');
  await page.click('#install');
  await expect(page.locator('#done-detail')).toHaveClass(/error/);
  await page.click('#finish');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'options');
  await expect(page.locator('#install')).toBeEnabled();
  await page.click('#install');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  assert.equal(
    await page.evaluate(
      () => window.__fixture.calls.filter((x) => x.command === 'run_install').length,
    ),
    2,
  );
  checks.push('failed install, preserved agreement, folder recheck and successful retry');
  await page.reload();
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'installed');
  await page.check('#agree');
  await page.fill('#folder', 'C:\\prism-ui-contract\\slow');
  await expect.poll(() => page.evaluate(() => window.__fixture.pending.length)).toBe(1);
  await page.fill('#folder', 'C:\\prism-ui-contract\\conflict');
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'conflict');
  await page.fill('#folder', 'C:\\prism-ui-contract\\slow');
  await expect.poll(() => page.evaluate(() => window.__fixture.pending.length)).toBe(2);
  await page.evaluate(() => window.__fixture.release(0, null));
  await expect(page.locator('#install')).toBeDisabled();
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'checking');
  await page.evaluate(() => window.__fixture.release(1, null));
  await expect(page.locator('#install')).toBeEnabled();
  await expect(page.locator('#repair')).toBeHidden();
  checks.push('A-B-A folder checks reject stale earlier replies; repair never targets old folder');
  await page.evaluate(() => {
    window.__fixture.healthOverride = 'empty';
  });
  await page.locator('#folder').fill('C:\\prism-ui-contract');
  await expect(page.locator('#location-state')).toHaveAttribute('data-state', 'ready');
  await expect(page.locator('#repair')).toBeHidden();
  checks.push('selected empty folder health overrides another registered installation version');
  async function fixtureMode(mode) {
    await page.evaluate((value) => sessionStorage.setItem('prism.fixture.mode', value), mode);
    await page.reload();
    await page.selectOption('#language', 'en');
    await expect(page.locator('.setup')).toHaveAttribute(
      'data-screen',
      mode === 'install' ? 'options' : mode,
    );
  }
  const lastArgs = (command) =>
    page.evaluate(
      (name) => window.__fixture.calls.filter((call) => call.command === name).at(-1)?.args,
      command,
    );
  await fixtureMode('install');
  await page.check('#agree');
  await page.evaluate(() => {
    window.__fixture.repairError = 'APP_RUNNING: fixture target process is alive';
  });
  await page.click('#repair');
  await expect(page.locator('#close-running-repair')).not.toBeChecked();
  await page.check('#repair-ack');
  await page.click('#repair-confirm');
  await expect(page.locator('#done-detail')).toContainText('tray menu');
  assert.equal((await lastArgs('run_repair')).closeRunningApp, false);
  await page.click('#finish');
  await expect(page.locator('#repair-dialog')).toBeVisible();
  await expect(page.locator('#repair-ack')).not.toBeChecked();
  await expect(page.locator('#close-running-repair')).not.toBeChecked();
  await page.evaluate(() => {
    window.__fixture.repairError = 'APP_CLOSE_FAILED: fixture close refused';
  });
  await page.check('#repair-ack');
  await page.check('#close-running-repair');
  await page.click('#repair-confirm');
  await expect(page.locator('#done-detail')).toContainText('after the close request');
  assert.equal((await lastArgs('run_repair')).closeRunningApp, true);
  await page.click('#finish');
  await expect(page.locator('#repair-dialog')).toBeVisible();
  await expect(page.locator('#repair-ack')).not.toBeChecked();
  await expect(page.locator('#close-running-repair')).not.toBeChecked();
  await page.evaluate(() => {
    window.__fixture.repairError = null;
    window.__fixture.suppressRepairProgress = true;
  });
  await page.check('#repair-ack');
  await page.check('#close-running-repair');
  await page.click('#repair-confirm');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'progress');
  await expect(page.locator('.setup')).not.toHaveClass(/failed/);
  await expect(page.locator('#stage')).toHaveText('Preparing');
  await expect(page.locator('#percent')).toHaveText('0%');
  await page.click('.screen-progress .detail-trigger');
  await expect(page.locator('#technical-error')).toBeHidden();
  await expect(page.locator('#technical-error-text')).toBeEmpty();
  await expect(page.locator('#log-text')).toContainText('Contract fixture');
  await page.keyboard.press('Escape');
  await page.selectOption('#language', 'ko');
  await expect(page.locator('#stage')).toHaveText('준비 중');
  checks.push('new attempt resets prior error and stage before native events; log history remains');
  await page.evaluate(() => window.__fixture.emitProgress({ stage: 'closing', fraction: 0 }));
  await expect(page.locator('#track')).not.toHaveAttribute('aria-valuenow', /.+/);
  await expect(page.locator('#percent')).toBeEmpty();
  for (const [language, label] of [
    ['ko', '실행 중인 Prism 정상 종료 중'],
    ['en', 'Closing the running Prism app'],
    ['zh-CN', '正在正常关闭 Prism'],
    ['zh-TW', '正在正常關閉 Prism'],
    ['ja', '実行中の Prism を正常終了中'],
  ]) {
    await page.selectOption('#language', language);
    await expect(page.locator('#stage')).toHaveText(label);
  }
  await page.setViewportSize({ width: 760, height: 520 });
  await page.selectOption('#theme', 'dark');
  await checkBounds(page, 'closing-stage-760-dark');
  await page.screenshot({ path: join(evidence, 'closing-760-dark.png') });
  await page.evaluate(() => window.__fixture.completeRepair());
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  assert.equal(await page.locator('#close-running-repair').isChecked(), false);
  checks.push(
    'repair close consent defaults false; refused close requires fresh repair acknowledgement; closing stage is localized and indeterminate',
  );

  await fixtureMode('install');
  await page.check('#agree');
  for (const [language, tray, registered] of [
    ['ko', '트레이', '등록된'],
    ['en', 'tray', 'registered'],
    ['zh-CN', '托盘', '已注册'],
    ['zh-TW', '系統匣', '已登錄'],
    ['ja', 'トレイ', '登録済み'],
  ])
    for (const code of ['APP_RUNNING', 'APP_CLOSE_FAILED', 'FOREIGN_INSTALLATION']) {
      await page.selectOption('#language', language);
      await expect(page.locator('#install')).toBeEnabled();
      const raw = `${code}: fixture original technical reason`;
      await page.evaluate((value) => {
        window.__fixture.installError = value;
      }, raw);
      await page.click('#install');
      await expect(page.locator('#done-detail')).toContainText(
        code === 'FOREIGN_INSTALLATION' ? registered : tray,
      );
      await expect(page.locator('#done-detail')).not.toContainText(code);
      assert.equal((await lastArgs('run_install')).closeRunningApp, false);
      await checkBounds(page, `${language}-${code}-760`);
      await page.click('.screen-done .detail-trigger');
      await expect(page.locator('#technical-error-text')).toHaveText(raw);
      await page.keyboard.press('Escape');
      const before = await page.evaluate(
        () => window.__fixture.calls.filter((call) => call.command === 'run_install').length,
      );
      await page.click('#finish');
      await expect(page.locator('#install')).toBeEnabled();
      await expect(page.locator('#close-running-install')).not.toBeChecked();
      assert.equal(
        await page.evaluate(
          () => window.__fixture.calls.filter((call) => call.command === 'run_install').length,
        ),
        before,
      );
      checks.push(
        `${language}-${code} localized guidance, exact technical details and non-mutating retry`,
      );
    }
  await page.evaluate(() => {
    window.__fixture.installError = null;
    window.__fixture.skipInstallFailure = true;
  });
  await page.check('#close-running-install');
  await page.click('#install');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  assert.equal((await lastArgs('run_install')).closeRunningApp, true);
  assert.equal(
    'closeRunningApp' in (await lastArgs('run_install')).options,
    false,
    'Close consent is a separate IPC argument.',
  );
  await expect(page.locator('#close-running-install')).not.toBeChecked();
  checks.push(
    'install transmits explicit separate close consent and consumes it for one operation',
  );

  await fixtureMode('uninstall');
  await expect(page.locator('#close-running-uninstall')).not.toBeChecked();
  await page.check('#remove-data');
  await page.evaluate(() => {
    window.__fixture.uninstallError = 'APP_RUNNING: fixture process alive';
  });
  await page.locator('#uninstall-form button[type="submit"]').click();
  await expect(page.locator('#done-detail')).toContainText('tray menu');
  assert.deepEqual(await lastArgs('run_uninstall'), { removeData: true, closeRunningApp: false });
  await page.click('#finish');
  await expect(page.locator('#remove-data')).toBeChecked();
  await expect(page.locator('#close-running-uninstall')).not.toBeChecked();
  await page.evaluate(() => {
    window.__fixture.uninstallError = 'APP_CLOSE_FAILED: fixture refused';
  });
  await page.check('#close-running-uninstall');
  await page.locator('#uninstall-form button[type="submit"]').click();
  await expect(page.locator('#done-detail')).toContainText('after the close request');
  assert.deepEqual(await lastArgs('run_uninstall'), { removeData: true, closeRunningApp: true });
  await page.click('#finish');
  await expect(page.locator('#close-running-uninstall')).not.toBeChecked();
  await page.evaluate(() => {
    window.__fixture.uninstallError = null;
  });
  await page.check('#close-running-uninstall');
  await page.locator('#uninstall-form button[type="submit"]').click();
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  checks.push(
    'uninstall preserves separate data choice while close consent is default false and fresh after failure',
  );

  for (const closeRunningApp of [false, true]) {
    await fixtureMode('restore');
    await expect(page.locator('#close-running-restore')).not.toBeChecked();
    await expect(page.locator('#restore')).toBeDisabled();
    await page.check('#restore-ack');
    if (closeRunningApp) await page.check('#close-running-restore');
    await page.click('#restore');
    await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
    assert.deepEqual(await lastArgs('run_restore'), {
      installDir: await page.evaluate(() => window.__fixture.directory),
      expectedVersion: '0.0.9',
      closeRunningApp,
    });
    await expect(page.locator('#close-running-restore')).not.toBeChecked();
  }
  checks.push(
    'restore keeps version/data acknowledgement separate from explicit optional close consent',
  );
  await fixtureMode('install');
  await page.evaluate(() => {
    window.__fixture.previousVersion = {
      currentVersion: '0.1.0',
      version: '0.0.9',
      requiredBytes: 1000,
    };
  });
  await page.selectOption('#language', 'ko');
  await expect(page.locator('#previous-options')).toBeVisible();
  await page.selectOption('#language', 'en');
  await page.click('#previous-options');
  await expect(page.locator('#restore-folder')).toHaveText('C:\\prism-ui-contract');
  await page.check('#restore-ack');
  await page.check('#close-running-restore');
  await page.click('#restore-back');
  await expect(page.locator('#restore-ack')).not.toBeChecked();
  await expect(page.locator('#close-running-restore')).not.toBeChecked();
  await page.evaluate(() => {
    window.__fixture.choosePath = 'C:\\prism-ui-contract\\B-slow';
  });
  await page.check('#close-running-install');
  await page.click('#browse');
  await expect.poll(() => page.evaluate(() => window.__fixture.pending.length)).toBe(1);
  await expect(page.locator('#previous-options')).toBeHidden();
  await expect(page.locator('#restore')).toBeDisabled();
  await expect(page.locator('#close-running-install')).not.toBeChecked();
  await page.evaluate(() =>
    window.__fixture.release(0, null, {
      currentVersion: '0.2.0',
      version: '0.0.8',
      requiredBytes: 1000,
    }),
  );
  await expect(page.locator('#previous-options')).toBeVisible();
  await page.click('#previous-options');
  await expect(page.locator('#restore-title')).toContainText('0.2.0');
  await expect(page.locator('#restore-title')).toContainText('0.0.8');
  await expect(page.locator('#restore-folder')).toHaveText('C:\\prism-ui-contract\\B-slow');
  await expect(page.locator('#restore-ack')).not.toBeChecked();
  await expect(page.locator('#close-running-restore')).not.toBeChecked();
  await checkBounds(page, 'restore-snapshot-760');
  await page.screenshot({ path: join(evidence, 'restore-snapshot-760.png') });
  checks.push('browse invalidates old restore target and both acknowledgements while checking');
  await page.check('#restore-ack');
  await page.check('#close-running-restore');
  const checksBeforeLanguage = await page.evaluate(
    () => window.__fixture.calls.filter((call) => call.command === 'check_folder').length,
  );
  await page.evaluate(() => {
    window.__fixture.previousVersion = {
      currentVersion: '9.0.0',
      version: '8.0.0',
      requiredBytes: 999999999,
    };
  });
  await page.selectOption('#language', 'ko');
  await expect(page.locator('#restore-title')).toContainText('0.2.0');
  await expect(page.locator('#restore-title')).toContainText('0.0.8');
  assert.equal(
    await page.evaluate(
      () => window.__fixture.calls.filter((call) => call.command === 'check_folder').length,
    ),
    checksBeforeLanguage,
  );
  await page.click('#restore');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  assert.deepEqual(await lastArgs('run_restore'), {
    installDir: 'C:\\prism-ui-contract\\B-slow',
    expectedVersion: '0.0.8',
    closeRunningApp: true,
  });
  checks.push('restore IPC uses the displayed path/version snapshot across language changes');

  await fixtureMode('restore');
  await page.evaluate(() => {
    window.__fixture.restoreError =
      'APP_RUNNING: fixture exact process. The previous installation was restored.';
  });
  await page.check('#restore-ack');
  await page.check('#close-running-restore');
  await page.click('#restore');
  await expect(page.locator('#done-detail')).toContainText('tray menu');
  await page.click('#finish');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'restore');
  await expect(page.locator('#restore-ack')).not.toBeChecked();
  await expect(page.locator('#close-running-restore')).not.toBeChecked();
  await expect(page.locator('#restore')).toBeDisabled();
  await page.evaluate(() => {
    window.__fixture.restoreError = null;
  });
  await page.check('#restore-ack');
  await page.click('#restore');
  await expect(page.locator('.setup')).toHaveAttribute('data-screen', 'done');
  assert.deepEqual(await lastArgs('run_restore'), {
    installDir: 'C:\\prism-ui-contract',
    expectedVersion: '0.0.9',
    closeRunningApp: false,
  });
  checks.push(
    'restore retry renews snapshot and mandatory acknowledgement without reusing close consent',
  );
  assert.deepEqual(errors, []);
  await writeFile(join(evidence, 'result.json'), JSON.stringify({ checks, errors }, null, 2));
  console.log(JSON.stringify({ passed: checks.length, errors, evidence }));
} catch (error) {
  await activePage?.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
  await writeFile(
    join(evidence, 'result.json'),
    JSON.stringify({ checks, errors, failure: String(error) }, null, 2),
  );
  console.error(`Setup UI evidence: ${evidence}`);
  throw error;
} finally {
  await browser?.close();
  await new Promise((r) => server.close(r));
}
