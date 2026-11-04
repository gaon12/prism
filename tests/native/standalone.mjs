import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, readFile, writeFile, rename, unlink, symlink } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';

const dataDir = await mkdtemp(join(tmpdir(), 'prism-standalone-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
const filesDir = join(dataDir, '문서');
await mkdir(filesDir);
const outsideDir = join(dataDir, '외부');
await mkdir(outsideDir);
await writeFile(join(outsideDir, '외부비밀.txt'), 'Outside the authorized scope');
await symlink(outsideDir, join(filesDir, '연결폴더'), 'junction');
for (const name of ['공문서 계획.hwpx', '회의 계획.txt', '프로젝트 계획.pdf'])
  await writeFile(join(filesDir, name), 'Prism filename test');
await Promise.all(
  Array.from({ length: 240 }, (_, i) =>
    writeFile(join(filesDir, `샘플 ${String(i).padStart(3, '0')}.txt`), ''),
  ),
);
await writeFile(
  join(dataDir, 'settings.json'),
  JSON.stringify({
    schemaVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F11',
    scopes: [
      {
        id: 'files',
        name: '테스트 문서',
        path: filesDir,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
const reservation = createServer();
await new Promise((done) => reservation.listen(0, '127.0.0.1', done));
const port = reservation.address().port;
await new Promise((done) => reservation.close(done));
const executable = resolve('src-tauri/target/debug/prism.exe');
const child = spawn(executable, [], {
  windowsHide: true,
  stdio: ['ignore', 'pipe', 'pipe'],
  env: {
    ...process.env,
    PRISM_DEV_DATA_DIR: dataDir,
    WEBVIEW2_USER_DATA_FOLDER: join(dataDir, 'webview'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
});
let stderr = '';
child.stderr.on('data', (chunk) => {
  stderr += chunk;
});
let browser;
try {
  const deadline = Date.now() + 25_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(stderr);
    try {
      if ((await fetch(`http://127.0.0.1:${port}/json/version`)).ok) break;
    } catch {
      /* Wait for WebView2. */
    }
    await new Promise((done) => setTimeout(done, 150));
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let pages = [];
  while (Date.now() < deadline) {
    pages = browser.contexts().flatMap((context) => context.pages());
    if (pages.filter((page) => page.url().includes('tauri.localhost')).length >= 2) break;
    await new Promise((done) => setTimeout(done, 150));
  }
  const main = pages.find(
    (page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'),
  );
  const quick = pages.find(
    (page) => page.url().includes('tauri.localhost') && page.url().includes('#quick'),
  );
  assert.ok(main && quick, 'Standalone windows must load bundled assets, not the Vite server.');
  const errors = [];
  for (const page of [main, quick]) page.on('pageerror', (error) => errors.push(error.message));
  await main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
  await main.screenshot({ path: join(evidence, 'standalone-main.png') });
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await quick.getByRole('textbox', { name: '검색어', exact: true }).waitFor();
  assert.equal(await quick.locator('.quick-empty').count(), 0);
  await until(async () => (await quick.evaluate(() => innerHeight)) <= 120, 'compact empty window');
  await quick.screenshot({ path: join(evidence, 'standalone-quick.png') });
  const quickInput = quick.getByRole('textbox', { name: '검색어', exact: true });
  await quickInput.fill('   ');
  assert.equal(await quick.locator('.quick-results').count(), 0);
  await quickInput.fill('계획');
  await until(
    async () => (await quick.getByRole('option').count()) === 3,
    'real quick filename results',
  );
  await until(
    async () => (await quick.evaluate(() => innerHeight)) >= 470,
    'expanded result window',
  );
  await quickInput.press('ArrowDown');
  const selected = await quick.locator('[role="option"][aria-selected="true"]').innerText();
  await quick.screenshot({ path: join(evidence, 'filename-quick.png') });
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Control+Enter');
  await until(async () => (await main.getByRole('option').count()) === 3, 'main filename handoff');
  assert.ok(
    (await main.locator('[role="option"][aria-selected="true"]').innerText()).startsWith(selected),
  );
  await main.screenshot({ path: join(evidence, 'filename-main.png') });
  const mainInput = main.getByRole('textbox', { name: '검색어', exact: true });
  await mainInput.fill('계획 extension:hwpx edit-date:>2023');
  await until(
    async () => (await main.getByRole('option').count()) === 1,
    'extension and date syntax',
  );
  assert.match(await main.getByRole('option').innerText(), /공문서 계획.hwpx/);
  await mainInput.fill('샘플');
  await until(async () => (await main.getByRole('option').count()) === 200, 'bounded results');
  assert.match(await main.locator('.results-heading').innerText(), /240/);
  await mainInput.fill('외부비밀');
  await main.getByRole('heading', { name: '검색 결과가 없어요' }).waitFor();
  await assert.rejects(
    main.evaluate(
      async (path) => window.__TAURI_INTERNALS__.invoke('file_action', { path, reveal: false }),
      join(outsideDir, '외부비밀.txt'),
    ),
    /outside the active file index/,
  );
  await mainInput.fill('새이름');
  await rename(join(filesDir, '회의 계획.txt'), join(filesDir, '새이름.txt'));
  await until(async () => (await main.getByRole('option').count()) === 1, 'live rename');
  await unlink(join(filesDir, '새이름.txt'));
  await until(async () => (await main.getByRole('option').count()) === 0, 'live deletion');
  await mainInput.fill('= 0.1+0.2');
  await until(
    async () => (await main.locator('.utility-value').textContent()) === '0.3',
    'bundled calculator worker and CSP',
  );
  await mainInput.fill('=48/2(9+3)');
  await until(
    async () => (await main.locator('.utility-value').textContent()) === '2',
    'implicit multiplication priority',
  );
  assert.match(
    await main.locator('.interpreted-expression').innerText(),
    /48 \/ \(2 \* \(9 \+ 3\)\)/,
  );
  await main.screenshot({ path: join(evidence, 'calculator-native.png') });
  await mainInput.fill('=48/2*(9+3)');
  await until(
    async () => (await main.locator('.utility-value').textContent()) === '288',
    'explicit multiplication stays left associative',
  );
  await mainInput.fill('unit: 20 km/L to L/100km');
  await until(
    async () => (await main.locator('.utility-value').textContent()) === '5 L/100km',
    'bundled unit conversion',
  );
  await main.getByRole('button', { name: '설정', exact: true }).click();
  await main.getByLabel('테마', { exact: true }).selectOption('dark');
  await main.getByLabel('글자 크기', { exact: true }).fill('16');
  await main.getByRole('button', { name: '단축키', exact: true }).click();
  await main.getByRole('textbox', { name: '단축키', exact: true }).fill('Ctrl+Alt+Shift+F11');
  await main.getByRole('button', { name: '저장', exact: true }).click();
  await main.getByRole('dialog', { name: '설정', exact: true }).waitFor({ state: 'hidden' });
  assert.equal(JSON.parse(await readFile(join(dataDir, 'settings.json'), 'utf8')).fontSize, 16);
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('독립 실행 확인');
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await quick.getByRole('heading', { name: '검색 결과가 없어요' }).waitFor();
  assert.equal(
    await quick.getByRole('textbox', { name: '검색어', exact: true }).inputValue(),
    '독립 실행 확인',
  );
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Control+Enter');
  assert.equal(
    await main.getByRole('textbox', { name: '검색어', exact: true }).inputValue(),
    '독립 실행 확인',
  );
  assert.deepEqual(errors, []);
  const exited = once(child, 'exit');
  await main.getByRole('button', { name: '설정', exact: true }).click();
  await main
    .getByRole('button', { name: 'Prism 종료', exact: true })
    .click()
    .catch((error) => {
      if (child.exitCode === null) throw error;
    });
  await exited;
  console.log(
    JSON.stringify(
      {
        result: 'passed',
        executable,
        dataDir,
        verified: [
          'bundled assets without Vite',
          'native settings UI and file',
          'native quick-window handoff',
          'dark theme and text size',
          'explicit quit',
          'compact empty and expanded result windows',
          'actual Korean filenames and selected result handoff',
          'extension/date filters and 200-result limit',
          'live rename and deletion',
          'junction isolation and out-of-scope open rejection',
          'bundled calculator and unit workers under production CSP',
          'implicit multiplication priority with displayed interpretation',
        ],
      },
      null,
      2,
    ),
  );
} finally {
  if (child.exitCode === null) {
    const stopped = once(child, 'exit');
    child.kill();
    await stopped;
  }
  if (browser?.isConnected()) await browser.close();
}

async function until(check, label) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(stderr || 'Prism exited.');
    try {
      if (await check()) return;
    } catch {
      /* Wait for UI or index. */
    }
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`Timed out: ${label}`);
}
