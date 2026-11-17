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
            await expect(previewPage.locator('#restore')).toBeDisabled();
            await previewPage.check('#restore-ack');
            await expect(previewPage.locator('#restore')).toBeEnabled();
          }
          if (screen === 'uninstall')
            await expect(previewPage.locator('#remove-data')).not.toBeChecked();
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
      calls: [],
      pending: [],
      logFailure: false,
      healthOverride: null,
      completeRepair: () => completeRepair(),
      release: (i, error) =>
        f.pending[i]({
          error,
          health: error ? 'conflict' : 'empty',
          freeBytes: 100000000,
          requiredBytes: 1000,
          repairAvailable: false,
          recoveryPending: false,
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
                mode: 'install',
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
              };
            case 'scan_installation':
              return { damaged: ['prism.exe'], checked: 3, total: 10, truncated: true };
            case 'run_repair':
              progress({ stage: 'copying', fraction: 0.4 });
              return new Promise((r) => {
                completeRepair = r;
              });
            case 'run_install':
              progress({ stage: 'copying', fraction: 0.6 });
              if (++installs === 1) throw 'Fixture install failure';
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
    { installDir: 'C:\\prism-ui-contract' },
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
