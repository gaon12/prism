// Search interactions in the actual app: Korean IME Enter, sorting that keeps the selection,
// cancelling a search, an empty result and a file that can no longer be opened.
//
// The IME part uses the WebView's own input pipeline (Chrome DevTools Protocol
// `Input.imeSetComposition` / `Input.insertText` / `Input.dispatchKeyEvent`), so the page
// receives trusted composition and key events rather than hand-built DOM events.
// Successful "open" requests are intercepted before they reach the OS, so no document app
// starts; the failed open goes through the real native command.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, rm, utimes, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32', 'This check requires Windows WebView2.');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the freshly built candidate executable.');
const expectedRelease = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(expectedRelease), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1');
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
async function hash(path) {
  const value = createHash('sha256');
  for await (const chunk of createReadStream(path)) value.update(chunk);
  return value.digest('hex');
}
const root = await mkdtemp(join(tmpdir(), 'prism-search-interactions-'));
const folder = join(root, 'files');
const application = join(root, 'app');
const data = join(application, 'PrismData');
const evidence = join(root, 'evidence');
await Promise.all([mkdir(folder), mkdir(data, { recursive: true }), mkdir(evidence)]);
// A copied executable with a portable marker never touches an installed profile.
const executable = join(application, 'prism.exe');
await copyFile(original, executable);
await writeFile(join(application, 'prism.portable'), '');
assert.equal(await hash(executable), await hash(original));

// Three matches with distinct sizes and dates, so each sort has one right answer.
const fixtures = [
  { name: 'q63-보고서-alpha.md', size: 30, day: 3 },
  { name: 'q63-보고서-beta.md', size: 10, day: 1 },
  { name: 'q63-보고서-gamma.md', size: 20, day: 2 },
];
for (const fixture of fixtures) {
  const path = join(folder, fixture.name);
  await writeFile(path, 'x'.repeat(fixture.size));
  const time = new Date(Date.UTC(2026, 0, fixture.day));
  await utimes(path, time, time);
}
const removable = join(folder, 'q63-삭제대상.md');
await writeFile(removable, 'removed before opening');
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F6',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    scopes: [
      {
        id: 'files',
        name: 'Files',
        path: folder,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);

// Record successful-open requests instead of sending them to the OS. `release` puts the
// original transport back so the failed-open step reaches the real command.
async function interceptOpens(page) {
  await page.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__opens = [];
    window.__releaseOpens = () => {
      window.fetch = original;
    };
    window.fetch = (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      const command = decodeURIComponent(url.pathname.slice(1));
      if (url.hostname !== 'ipc.localhost' || command !== 'file_action')
        return original(input, options);
      const raw = options?.body;
      window.__opens.push(
        JSON.parse(typeof raw === 'string' ? raw : new TextDecoder().decode(raw)).path,
      );
      return Promise.resolve(
        new Response('null', {
          headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
        }),
      );
    };
  });
}
const opens = (page) => page.evaluate(() => window.__opens);
const selectedName = (page) =>
  page.locator('.file-result-row[aria-selected="true"] .file-result-title').textContent();
const shownNames = (page) => page.locator('.file-result-row .file-result-title').allTextContents();

const checks = [];
let instance;
try {
  instance = await launch(data, {
    executable,
    portable: true,
    productionPortable: expectedRelease === '1',
    cdpPort: process.env.PRISM_APP_CDP_PORT ? Number(process.env.PRISM_APP_CDP_PORT) : undefined,
  });
  const { main, invoke } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.development, expectedRelease !== '1');
  assert.equal(runtime.installKind, 'portable');
  await until(async () => (await invoke('index_status')).initialScanComplete, 'fixture scan');
  await interceptOpens(main);
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  const input = main.getByRole('textbox', { name: '검색어', exact: true });

  // 1. Korean IME: Enter while a syllable is still being composed must not open anything.
  const cdp = await main.context().newCDPSession(main);
  await input.fill('q63-');
  await input.focus();
  await cdp.send('Input.imeSetComposition', { text: '보고', selectionStart: 2, selectionEnd: 2 });
  await expect(input).toHaveValue('q63-보고');
  // During composition the WebView reports the Enter key as the IME's (key code 229).
  for (const type of ['rawKeyDown', 'keyUp'])
    await cdp.send('Input.dispatchKeyEvent', {
      type,
      key: 'Enter',
      code: 'Enter',
      windowsVirtualKeyCode: 229,
      nativeVirtualKeyCode: 229,
    });
  await main.waitForTimeout(300);
  assert.deepEqual(await opens(main), [], 'Enter during composition opens nothing');
  // Committing the composition ends it; the next Enter opens the selected result.
  await cdp.send('Input.insertText', { text: '보고서' });
  await expect(input).toHaveValue('q63-보고서');
  await expect(main.locator('.file-result-row')).toHaveCount(3);
  await input.press('Enter');
  await until(async () => (await opens(main)).length === 1, 'open after composition');
  // Compare by name: the runner's temp folder may be reported in another spelling (8.3).
  const opened = (await opens(main))[0];
  assert(opened.endsWith('\\' + (await selectedName(main))), 'opened the selection');
  checks.push('imeEnter');

  // 2. Sorting keeps the selected file selected and moves the keyboard in the new order.
  await main.locator('.file-result-row').filter({ hasText: 'q63-보고서-beta.md' }).click();
  assert.equal(await selectedName(main), 'q63-보고서-beta.md');
  const size = main.getByRole('button', { name: '크기', exact: true });
  await size.click();
  await expect(size).toHaveAttribute('aria-pressed', 'true');
  assert.deepEqual(await shownNames(main), [
    'q63-보고서-beta.md',
    'q63-보고서-gamma.md',
    'q63-보고서-alpha.md',
  ]);
  assert.equal(await selectedName(main), 'q63-보고서-beta.md', 'selection kept after sort');
  await size.click();
  assert.deepEqual(await shownNames(main), [
    'q63-보고서-alpha.md',
    'q63-보고서-gamma.md',
    'q63-보고서-beta.md',
  ]);
  assert.equal(await selectedName(main), 'q63-보고서-beta.md', 'selection kept after reverse');
  const list = main.locator('.file-result-list');
  await list.focus();
  await list.press('ArrowUp');
  assert.equal(await selectedName(main), 'q63-보고서-gamma.md', 'arrow keys follow the order');
  await main.getByRole('button', { name: '수정일', exact: true }).click();
  assert.deepEqual(await shownNames(main), [
    'q63-보고서-beta.md',
    'q63-보고서-gamma.md',
    'q63-보고서-alpha.md',
  ]);
  assert.equal(await selectedName(main), 'q63-보고서-gamma.md');
  // A third press of a heading returns to best-match order.
  const name = main.getByRole('button', { name: '이름', exact: true });
  for (let press = 0; press < 3; press++) await name.click();
  await expect(name).toHaveAttribute('aria-pressed', 'false');
  assert.equal(await selectedName(main), 'q63-보고서-gamma.md');
  await main.screenshot({ path: join(evidence, 'sorted.png') });
  checks.push('sortKeepsSelection');

  // 3. Cancelling: Escape clears the search, and a replaced query never shows stale rows.
  await input.focus();
  await input.press('Escape');
  await expect(input).toHaveValue('');
  await expect(main.locator('.file-result-row')).toHaveCount(0);
  await input.fill('q63-보고서-alpha');
  await input.fill('q63-보고서-beta');
  await expect(main.locator('.file-result-row')).toHaveCount(1);
  await main.waitForTimeout(500);
  assert.deepEqual(await shownNames(main), ['q63-보고서-beta.md'], 'only the latest query');
  checks.push('cancel');

  // 4. An empty result explains itself instead of showing an empty list.
  await input.fill('q63-없는검색어-zzzz');
  await expect(main.getByText('검색 결과가 없어요', { exact: true })).toBeVisible();
  await expect(main.locator('.file-result-row')).toHaveCount(0);
  await main.screenshot({ path: join(evidence, 'empty.png') });
  checks.push('emptyResult');

  // 5. A file deleted after it was listed: the real open command refuses and says why.
  await main.evaluate(() => window.__releaseOpens());
  await input.fill('q63-삭제대상');
  const row = main.locator('.file-result-row').filter({ hasText: 'q63-삭제대상.md' });
  await row.waitFor();
  await input.focus();
  // The watcher may drop the row moments after the delete; press Enter right away.
  await rm(removable);
  await input.press('Enter');
  const toast = main.locator('.toast');
  await expect(toast).toContainText('파일이 더 이상 없거나 열 수 없습니다');
  await main.screenshot({ path: join(evidence, 'open-failed.png') });
  checks.push('openFailure');

  assert.deepEqual(instance.errors, []);
  console.log(JSON.stringify({ passed: true, checks, root, release: expectedRelease === '1' }));
} finally {
  if (instance) await shutdown(instance);
}
