// Real isolated settings, filename/content indexes and bundled Windows UI.
// Browser plugin not available: own WebView/Playwright. Only the final OS open/copy
// calls are intercepted, so this never launches a document app or changes the clipboard.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert(
  process.env.PRISM_TEST_EXECUTABLE,
  'Supply the freshly built candidate, not an older default.',
);
assert.equal(process.platform, 'win32', 'This check requires Windows WebView2.');
const requestedProfile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(requestedProfile), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1.');
const expectRelease = requestedProfile === '1';
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const directory = await mkdtemp(join(tmpdir(), 'prism-file-shortcuts-'));
// A release binary ignores PRISM_DEV_DATA_DIR. Isolate BEFORE launching, even if
// someone accidentally supplies release instead of the intended debug candidate.
const portable = join(directory, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
const executableSha256 = createHash('sha256')
  .update(await readFile(original))
  .digest('hex');
assert.equal(
  createHash('sha256')
    .update(await readFile(executable))
    .digest('hex'),
  executableSha256,
);
await writeFile(join(portable, 'prism.portable'), '');
await mkdir(join(portable, 'notices'));
for (const name of ['pdf-renderer', 'document-pdf', 'document-hwpx', 'office-passwords']) {
  const notice = join(dirname(original), 'notices', `${name}.txt`);
  const target = join(portable, 'notices', `${name}.txt`);
  await copyFile(notice, target);
  assert.deepEqual(await readFile(target), await readFile(notice));
}
const folder = join(directory, 'files');
const document = join(folder, 'needle-document.md');
const childFolder = join(folder, 'needle-folder');
await mkdir(childFolder, { recursive: true });
await writeFile(document, '# 단축키\n본문검색전용문장 keyword opening fixture.');
const settingsPath = join(data, 'settings.json');
await writeFile(
  settingsPath,
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F7',
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    scopes: [
      {
        id: 'files',
        name: 'Files',
        path: folder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);

async function captureActions(page) {
  await page.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__fileShortcutActions = [];
    window.fetch = (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1));
      if (
        ['file_action', 'content_file_action', 'content_open_location', 'copy_text'].includes(
          command,
        )
      ) {
        const raw = options?.body;
        const args = JSON.parse(typeof raw === 'string' ? raw : new TextDecoder().decode(raw));
        window.__fileShortcutActions.push({ command, args });
        return Promise.resolve(
          new Response(JSON.stringify(command === 'content_open_location' ? true : null), {
            headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
          }),
        );
      }
      return original(input, options);
    };
  });
}
const actions = (page) => page.evaluate(() => window.__fileShortcutActions);
const clear = (page) =>
  page.evaluate(() => {
    window.__fileShortcutActions.length = 0;
  });
async function noAction(page, element, key) {
  await clear(page);
  await element.press(key);
  await page.waitForTimeout(200);
  assert.deepEqual(await actions(page), []);
}
async function openWith(page, element, key, expected) {
  await clear(page);
  await element.press(key);
  await until(async () => (await actions(page)).length > 0, 'configured opening action');
  assert.deepEqual(await actions(page), [expected]);
}

let instance;
try {
  instance = await launch(data, { executable, portable: true });
  let { main, quick, invoke } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(
    runtime.development,
    !expectRelease,
    'Candidate profile must match the requested QA profile.',
  );
  assert.equal(runtime.installKind, 'portable');
  assert.equal((await invoke('get_settings')).fileOpenShortcut, 'Enter', 'legacy native default');
  await until(async () => (await invoke('index_status')).initialScanComplete, 'fixture filenames');
  await until(async () => (await invoke('content_status')).indexed >= 1, 'real fixture extraction');
  for (const page of [main, quick]) await captureActions(page);
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('needle-document');
  await main.getByRole('option').filter({ hasText: 'needle-document.md' }).waitFor();
  await openWith(main, input, 'Enter', {
    command: 'file_action',
    args: { path: document, reveal: false },
  });

  await main.getByRole('button', { name: '설정', exact: true }).click();
  await main.getByRole('button', { name: '단축키', exact: true }).click();
  await main.getByLabel('파일·폴더 열기', { exact: true }).fill('Ctrl+O');
  await main.getByRole('button', { name: '저장', exact: true }).click();
  await until(
    async () => (await invoke('get_settings')).fileOpenShortcut === 'Ctrl+O',
    'native key persisted',
  );
  assert.equal(JSON.parse(await readFile(settingsPath, 'utf8')).fileOpenShortcut, 'Ctrl+O');
  await expect(main.locator('.search-box kbd')).toHaveText('Ctrl+O');
  await noAction(main, input, 'Enter');
  await openWith(main, input, 'Control+o', {
    command: 'file_action',
    args: { path: document, reveal: false },
  });
  // Real DOM key events with IME composition and legacy keyCode 229 must not open.
  for (const extra of [{ isComposing: true }, { keyCode: 229 }, { repeat: true }]) {
    await clear(main);
    const consumed = await input.evaluate(
      (element, extra) =>
        !element.dispatchEvent(
          new KeyboardEvent('keydown', {
            key: 'ㅐ',
            code: 'KeyO',
            ctrlKey: true,
            bubbles: true,
            cancelable: true,
            ...extra,
          }),
        ),
      extra,
    );
    assert.equal(
      consumed,
      extra.repeat === true,
      'held shortcuts are swallowed; IME input is left alone',
    );
    await main.waitForTimeout(120);
    assert.deepEqual(await actions(main), []);
  }
  await clear(main);
  await input.dispatchEvent('keydown', { key: 'ㅐ', code: 'KeyO', ctrlKey: true, bubbles: true });
  await until(
    async () => (await actions(main)).length === 1,
    'physical Latin key under Korean IME',
  );
  assert.equal((await actions(main))[0].args.path, document);

  await input.fill('needle-folder');
  await main.getByRole('option').filter({ hasText: 'needle-folder' }).click();
  const files = main.locator('.file-result-list');
  await expect(files).toBeFocused();
  await noAction(main, files, 'Enter');
  await openWith(main, files, 'Control+o', {
    command: 'file_action',
    args: { path: childFolder, reveal: false },
  });

  // Reject hand-edited IPC payloads too, without changing the persisted profile.
  const saved = await invoke('get_settings');
  const before = await readFile(settingsPath, 'utf8');
  for (const [key, error] of [
    ['Ctrl+Enter', 'fileShortcutReserved'],
    [saved.shortcut, 'fileShortcutConflict'],
  ]) {
    await assert.rejects(
      invoke('save_settings', {
        settings: { ...saved, fileOpenShortcut: key },
        expectedRevision: saved.revision,
      }),
      (cause) => String(cause).includes(error),
    );
    assert.equal(await readFile(settingsPath, 'utf8'), before);
  }

  await main.getByRole('button', { name: '본문', exact: true }).click();
  await input.fill('본문검색전용문장');
  const contentList = main.locator('.content-result-list');
  await contentList.getByRole('option').waitFor();
  const response = await invoke('search_content', {
    query: '본문검색전용문장',
    extension: '',
    scopeId: 'files',
    requestId: 'file-shortcut-location',
  });
  const chunkId = response.hits[0].chunk.id;
  await openWith(main, input, 'Control+o', {
    command: 'content_file_action',
    args: { path: document, reveal: false },
  });
  await contentList.focus();
  await noAction(main, contentList, 'Enter');
  await openWith(main, contentList, 'Control+o', {
    command: 'content_open_location',
    args: { chunkId },
  });

  // Other window receives the actual settings event; utilities retain Enter.
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  await input.fill('needle-document');
  await invoke('show_quick');
  const quickInput = quick.getByRole('textbox', { name: '검색어', exact: true });
  await quick.getByRole('option').filter({ hasText: 'needle-document.md' }).waitFor();
  await expect(quick.locator('.quick-footer')).toContainText('Ctrl+O');
  await noAction(quick, quickInput, 'Enter');
  await openWith(quick, quickInput, 'Control+o', {
    command: 'file_action',
    args: { path: document, reveal: false },
  });
  await invoke('show_quick');
  await quickInput.fill('=1+1');
  await quick.locator('output').filter({ hasText: '2' }).waitFor();
  await noAction(quick, quickInput, 'Control+o');
  await openWith(quick, quickInput, 'Enter', { command: 'copy_text', args: { text: '2' } });
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  assert.deepEqual(instance.errors, []);
  await main.screenshot({ path: join(directory, 'custom-shortcut.png') });
  await shutdown(instance);
  instance = undefined;

  instance = await launch(data, { executable, portable: true });
  assert.equal((await instance.invoke('get_settings')).fileOpenShortcut, 'Ctrl+O');
  await expect(instance.main.locator('.search-box kbd')).toHaveText('Enter'); // remembered calculator input
  await instance.main.getByRole('textbox', { name: '검색어', exact: true }).fill('needle');
  await expect(instance.main.locator('.search-box kbd')).toHaveText('Ctrl+O');
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      executable,
      original,
      executableSha256,
      release: expectRelease,
      nativePersistence: true,
      finalOsActionsIntercepted: true,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
