// Actual reset UI, IPC, startup journal, database reopen and app restart. Only
// generated data in a private portable copy is used; no user profile/service.
// Browser plugin not available: existing Playwright/WebView2 harness.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { launch, shutdown, until, WEBVIEW_START_TIMEOUT } from './harness.mjs';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the new candidate executable');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1');
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const root = await mkdtemp(join(tmpdir(), 'prism-index-reset-native-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
writePackageNotices(portable, loadPackageNotices(original));
await writeFile(join(portable, 'prism.portable'), '');
async function hash(path) {
  const h = createHash('sha256');
  for await (const part of createReadStream(path)) h.update(part);
  return h.digest('hex');
}
const executableSha256 = await hash(original);
assert.equal(await hash(executable), executableSha256);
const source = join(root, 'sources');
await mkdir(source);
const sourceFile = join(source, 'resetneedle.md');
await writeFile(sourceFile, '# resetneedle\nOriginal fixture document stays unchanged.');
const sourceHash = await hash(sourceFile);
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    language: 'ko',
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    rememberSearch: true,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'fixture',
        name: '보존할 원본',
        path: source,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
// A valid generated clipboard record, not the OS clipboard.
const clipText = '보존할 클립보드 기록';
const clipId = createHash('sha256').update(`text\0${clipText}`).digest('hex');
await mkdir(join(data, 'clipboard', 'blobs'), { recursive: true });
await writeFile(join(data, 'clipboard', 'blobs', `${clipId}.txt`), clipText);
await writeFile(
  join(data, 'clipboard', 'index.json'),
  JSON.stringify({
    version: 1,
    items: [
      {
        id: clipId,
        hash: clipId,
        kind: 'text',
        preview: clipText,
        byteSize: Buffer.byteLength(clipText),
        createdMs: Date.now(),
        lastCopiedMs: Date.now(),
        copyCount: 1,
        pinned: false,
      },
    ],
  }),
);
await mkdir(join(data, 'embedding'), { recursive: true });
const modelSentinel = join(data, 'embedding', 'preserved-fixture.txt');
await writeFile(modelSentinel, 'model-area sentinel; not an actual model');
let instance;
let restarted = false;
let summary;
const errors = [];
const screenshots = [];
const resultPath = process.env.PRISM_NATIVE_RESULT ?? join(root, 'result.json');
const isAbsent = async (path) => {
  try {
    await stat(path);
    return false;
  } catch (error) {
    if (error.code === 'ENOENT') return true;
    throw error;
  }
};
const callResult = (page, command, args) =>
  page.evaluate(
    async ({ command, args }) => {
      try {
        return { value: await window.__TAURI_INTERNALS__.invoke(command, args) };
      } catch (error) {
        return { error: typeof error === 'string' ? error : (error?.message ?? String(error)) };
      }
    },
    { command, args },
  );
try {
  instance = await launch(data, { executable, portable: true });
  let { main, invoke } = instance;
  assert.equal((await invoke('runtime_info')).installKind, 'portable');
  assert.equal((await invoke('runtime_info')).development, profile === '0');
  assert.equal((await invoke('index_reset_status')).lastApplied, false);
  await until(async () => {
    const s = await invoke('content_status');
    return s.phase === 'ready' && s.indexed === 1;
  }, 'initial content indexing');
  const contentSentinel = join(data, 'content', 'old-cache-proof.txt');
  await writeFile(contentSentinel, 'must disappear only after confirmed content reset');
  const chat = await invoke('rag_create', { title: '보존할 대화' });
  const document = await invoke('doc_create', {
    input: {
      title: '보존할 초안',
      purpose: 'resetneedle',
      scopeIds: ['fixture'],
      templateMarkdown: '보존할 양식',
    },
  });
  const collection = await invoke('collection_create', {
    title: '보존할 컬렉션',
    description: 'reference only',
  });
  const session = await invoke('get_search_session');
  await invoke('set_search_session', { session: { ...session, query: 'resetneedle' } });
  const settings = await invoke('get_settings');
  const settingsHash = await hash(join(data, 'settings.json'));
  const rejected = await callResult(main, 'reset_index', {
    target: 'filename',
    expectedRevision: settings.revision + 1,
  });
  assert(rejected.error?.includes('SETTINGS_CHANGED'));
  const quickRejected = await callResult(instance.quick, 'reset_index', {
    target: 'content',
    expectedRevision: settings.revision,
  });
  assert(quickRejected.error?.includes('main window'));
  assert(await isAbsent(join(data, 'index-reset.json')));
  await invoke('open_settings', { tab: 'scopes' });
  const settingsDialog = main.locator('.settings-modal[open]');
  const panel = settingsDialog.getByRole('region', { name: '색인 초기화', exact: true });
  const filenames = panel.getByRole('button', { name: '파일명 색인 다시 만들기', exact: true });
  const content = panel.getByRole('button', { name: '본문·의미 색인 초기화…', exact: true });
  await filenames.waitFor();
  await settingsDialog.locator('.scope-checkboxes input').first().uncheck();
  await until(
    async () => (await filenames.isDisabled()) && (await content.isDisabled()),
    'unsaved settings block reset',
  );
  await main.keyboard.press('Escape');
  await invoke('open_settings', { tab: 'scopes' });
  await filenames.click();
  const confirmation = main.locator('.index-reset-dialog[open]');
  await confirmation.getByRole('button', { name: '취소', exact: true }).click();
  assert(await settingsDialog.isVisible(), 'closing nested confirmation keeps settings open');
  assert(!(await isAbsent(contentSentinel)));
  await filenames.click();
  await confirmation.getByRole('button', { name: '목록 비우고 다시 수집', exact: true }).click();
  await panel.getByText('파일명 목록을 다시 수집하고 있습니다.', { exact: true }).waitFor();
  await until(async () => {
    const s = await invoke('index_status');
    return !s.scanning && s.initialScanComplete;
  }, 'filename rebuild completes');
  assert(
    (
      await invoke('search_files', { query: 'resetneedle', extension: '', scopeId: null })
    ).results.some((file) => file.name === 'resetneedle.md'),
  );
  assert(!(await isAbsent(contentSentinel)), 'filename rebuild keeps content cache');
  await main.keyboard.press('Escape');
  // Unsaved document creation must block only the restart action.
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main.getByRole('button', { name: '새 문서', exact: true }).click();
  const title = main.getByLabel('문서 제목', { exact: true });
  await title.fill('저장하지 않은 문서');
  await invoke('open_settings', { tab: 'scopes' });
  await until(async () => await content.isDisabled(), 'unsaved document blocks restart');
  assert.equal(await filenames.isEnabled(), true, 'filename rebuild does not restart the editor');
  await main.keyboard.press('Escape');
  await title.fill('');
  await invoke('open_settings', { tab: 'scopes' });
  await until(async () => await content.isEnabled(), 'cleared unsaved document allows restart');
  await content.click();
  await confirmation.getByText(/OCR로 추출했던 본문도 지웁니다/).waitFor();
  await confirmation.getByRole('button', { name: '취소', exact: true }).click();
  assert(await isAbsent(join(data, 'index-reset.json')));
  assert(!(await isAbsent(contentSentinel)));
  await content.click();
  screenshots.push(join(root, 'reset-confirmation.png'));
  await main.screenshot({ path: screenshots.at(-1) });
  const port = instance.port;
  const oldChild = instance.child;
  restarted = true;
  await confirmation
    .getByRole('button', { name: '초기화하고 다시 시작', exact: true })
    .click()
    .catch((error) => {
      if (!/closed|disconnected/i.test(String(error))) throw error;
    });
  await until(
    () => oldChild.exitCode !== null,
    'original app exits for reset',
    WEBVIEW_START_TIMEOUT,
  );
  await instance.browser.close().catch(() => {});
  await until(
    async () => {
      try {
        return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
      } catch {
        return false;
      }
    },
    'restarted app endpoint',
    WEBVIEW_START_TIMEOUT,
  );
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let quick;
  await until(() => {
    const pages = browser.contexts().flatMap((context) => context.pages());
    main = pages.find(
      (page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'),
    );
    quick = pages.find((page) => page.url().includes('#quick'));
    return main && quick;
  }, 'restarted app windows');
  for (const page of [main, quick]) page.on('pageerror', (error) => errors.push(error.message));
  invoke = (command, args = {}) =>
    main.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  instance = { ...instance, browser, main, quick, invoke };
  await main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
  const resetStatus = await invoke('index_reset_status');
  assert.deepEqual(resetStatus, { lastApplied: true, pending: false, error: null });
  assert(await isAbsent(contentSentinel));
  assert(await isAbsent(join(data, 'index-reset.json')));
  assert.equal(await hash(sourceFile), sourceHash);
  assert.equal(await hash(join(data, 'settings.json')), settingsHash);
  assert.equal((await invoke('get_search_session')).query, 'resetneedle');
  assert.equal(
    (await invoke('rag_conversation', { id: chat.id, beforeSequence: null, limit: 100 }))
      .conversation.title,
    '보존할 대화',
  );
  assert.equal(
    (await invoke('doc_get', { id: document.document.id, revisionId: null })).document.title,
    '보존할 초안',
  );
  assert.equal((await invoke('collection_get', { id: collection.id })).title, '보존할 컬렉션');
  assert.equal(await invoke('clipboard_text', { id: clipId }), clipText);
  assert.equal(await readFile(modelSentinel, 'utf8'), 'model-area sentinel; not an actual model');
  await until(async () => {
    const s = await invoke('content_status');
    return s.phase === 'ready' && s.indexed === 1;
  }, 'content reindexes after reset');
  const result = await invoke('search_content', {
    query: 'resetneedle',
    extension: '',
    scopeId: null,
    requestId: crypto.randomUUID(),
  });
  assert(result.hits.some((hit) => hit.chunk.filePath.endsWith('resetneedle.md')));
  await invoke('open_settings', { tab: 'scopes' });
  const completedNotice = main.getByText('본문·의미 색인을 초기화했습니다.', { exact: true });
  await completedNotice.waitFor();
  await completedNotice.scrollIntoViewIfNeeded();
  screenshots.push(join(root, 'reset-completed.png'));
  await main.screenshot({ path: screenshots.at(-1) });
  // A second confirmation is cancelled: verify compact dark/English layout
  // without clearing data or restarting again.
  await main.keyboard.press('Escape');
  const visualSettings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...visualSettings, language: 'en', theme: 'dark' },
    expectedRevision: visualSettings.revision,
  });
  await main.setViewportSize({ width: 390, height: 844 });
  await invoke('open_settings', { tab: 'scopes' });
  await main
    .getByRole('button', { name: 'Reset content and semantic indexes…', exact: true })
    .click();
  const compactDialog = main.locator('.index-reset-dialog[open]');
  await compactDialog.getByRole('button', { name: 'Reset and restart', exact: true }).waitFor();
  assert(
    await compactDialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1),
    'confirmation has no horizontal overflow',
  );
  screenshots.push(join(root, 'reset-compact-en-dark.png'));
  await main.screenshot({ path: screenshots.at(-1) });
  await compactDialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  assert(await isAbsent(join(data, 'index-reset.json')));
  assert.deepEqual(instance.errors, []);
  assert.deepEqual(errors, []);
  summary = {
    passed: true,
    root,
    executableSha256,
    development: profile === '0',
    testSha256: await hash(fileURLToPath(import.meta.url)),
    actualRestart: true,
    filenameRebuild: true,
    staleRevisionRejected: true,
    quickResetRejected: true,
    unsavedSettingsAndDocumentProtected: true,
    contentReindexed: true,
    originalSettingsSessionChatsDocumentsCollectionsClipboardModelsPreserved: true,
    screenshots,
    errors: [],
  };
  console.log(JSON.stringify(summary));
} catch (error) {
  if (instance) {
    await instance.main.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
    await writeFile(
      join(root, 'failure-ui.txt'),
      await instance.main
        .locator('body')
        .innerText()
        .catch(() => ''),
    ).catch(() => {});
  }
  throw error;
} finally {
  if (instance) await shutdown(instance).catch(() => {});
  // A restart creates a new OS child. Only this exact copied executable may be
  // cleaned up if an assertion interrupted reconnection; never process names.
  if (restarted)
    spawnSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-Command',
        'Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $env:PRISM_RESET_OWNED_EXE } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }',
      ],
      {
        windowsHide: true,
        env: { ...process.env, PRISM_RESET_OWNED_EXE: executable },
        encoding: 'utf8',
      },
    );
  await writeFile(
    resultPath,
    JSON.stringify(summary ?? { passed: false, root, executableSha256 }, null, 2),
  );
}
