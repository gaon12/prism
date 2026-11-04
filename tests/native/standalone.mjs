import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, mkdtemp, readFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';

const dataDir = await mkdtemp(join(tmpdir(), 'prism-standalone-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
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
  await quick.getByRole('heading', { name: '검색할 폴더가 없어요' }).waitFor();
  await quick.screenshot({ path: join(evidence, 'standalone-quick.png') });
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Control+Enter');
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
  await quick.getByRole('heading', { name: '검색할 폴더가 없어요' }).waitFor();
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
