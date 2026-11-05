// Real bundled WebView -> installed Windows OCR -> SQLite -> actual Q8/Lance.
// Browser plugin not available: repository Playwright/WebView2 harness.
// All documents, processes and profiles belong to this test; no OS clipboard.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-ocr-'));
const folder = join(directory, 'allowed');
const privateFolder = join(directory, 'filename-only');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all([folder, privateFolder, evidence].map((p) => mkdir(p, { recursive: true })));
const names = ['bilingual.png', 'transparent.png', 'scan.pdf', 'mixed.pdf', 'scan-locked.pdf'];
await Promise.all(names.map((name) => copyFile(`tests/fixtures/ocr/${name}`, join(folder, name))));
await copyFile('tests/fixtures/ocr/bilingual.png', join(privateFolder, 'private.png'));
const locked = join(folder, 'scan-locked.pdf');
const original = await readFile(locked);
const sha = createHash('sha256').update(original).digest('hex');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      { id: 'ocr', name: 'OCR', path: folder, filenameEnabled: true, contentEnabled: true },
      {
        id: 'private',
        name: 'Private',
        path: privateFolder,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
const options = process.env.PRISM_TEST_EXECUTABLE
  ? { executable: process.env.PRISM_TEST_EXECUTABLE }
  : {};
let instance;
const search = (query = '74829') =>
  instance.invoke('search_content', {
    query,
    extension: '',
    scopeId: null,
    requestId: crypto.randomUUID(),
  });
const waitReady = () =>
  until(
    async () => (await instance.invoke('content_status')).phase === 'ready',
    'content worker ready',
  );
const save = async (mutate) => {
  const settings = await instance.invoke('get_settings');
  mutate(settings);
  return instance.invoke('save_settings', { settings, expectedRevision: settings.revision });
};
const openOcrSettings = async () => {
  await instance.invoke('open_settings', { tab: 'scopes' });
  const dialog = instance.main.getByRole('dialog', { name: '설정', exact: true });
  await dialog.getByRole('heading', { name: '이미지·스캔 문서 OCR', exact: true }).waitFor();
  return dialog;
};
try {
  instance = await launch(directory, options);
  let { main, invoke } = instance;
  const capabilities = await invoke('content_ocr_capabilities');
  assert(capabilities.available && capabilities.pdfAvailable);
  assert.equal(capabilities.engine, 'windowsNative');
  assert(capabilities.languages.some((l) => l.tag === 'ko'));
  await waitReady();
  assert.equal((await invoke('get_settings')).ocr.enabled, false, 'OCR defaults off');
  assert.equal((await search()).hits.length, 0, 'no reading raster text before opt-in');
  assert.equal((await search('original')).hits.length, 2, 'typed pages survive without OCR');
  console.log(JSON.stringify({ directory, phase: 'default-off-and-typed-pages' }));
  let dialog = await openOcrSettings();
  const toggleName = '새 이미지·스캔 페이지의 글자 읽기';
  assert.equal(await dialog.getByRole('checkbox', { name: toggleName }).isChecked(), false);
  await dialog.getByRole('status').filter({ hasText: 'Windows 내장 OCR' }).waitFor();
  await dialog.getByLabel('OCR 언어', { exact: true }).selectOption('ko');
  await dialog.getByRole('checkbox', { name: toggleName }).check();
  for (const [theme, width] of [
    ['light', 1280],
    ['dark', 390],
  ]) {
    await main.setViewportSize({ width, height: 840 });
    await main.evaluate((theme) => (document.documentElement.dataset.theme = theme), theme);
    await dialog
      .getByRole('heading', { name: '이미지·스캔 문서 OCR', exact: true })
      .scrollIntoViewIfNeeded();
    assert(await dialog.evaluate((el) => el.getBoundingClientRect().right <= innerWidth + 1));
    assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await main.screenshot({ path: join(evidence, `ocr-settings-${theme}-${width}.png`) });
  }
  await main.setViewportSize({ width: 1600, height: 1050 });
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 4;
  }, 'four actually OCR-indexed documents');
  let hits = (await search()).hits;
  assert.equal(new Set(hits.map((h) => h.chunk.filePath)).size, 4);
  assert(hits.every((h) => !h.chunk.filePath.includes('filename-only')));
  assert(
    hits.filter((h) => h.chunk.filePath.endsWith('mixed.pdf')).every((h) => h.chunk.page === 2),
  );
  assert.equal(
    (await invoke('model_status')).phase,
    'notInstalled',
    'OCR does not install/start an embedding model',
  );
  console.log(JSON.stringify({ directory, phase: 'actual-ocr-indexed' }));
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.locator('.content-issues summary').click();
  await main.getByRole('button', { name: '암호 입력: scan-locked.pdf', exact: true }).click();
  const unlock = main.getByRole('dialog', { name: '암호 PDF 다시 색인', exact: true });
  await unlock.getByLabel('PDF 비밀번호', { exact: true }).fill('ocr-fixture-password');
  await unlock.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await unlock.waitFor({ state: 'hidden' });
  hits = (await search()).hits;
  assert.equal(new Set(hits.map((h) => h.chunk.filePath)).size, 5);
  assert(hits.some((h) => h.chunk.fingerprint.sha256 === sha && h.chunk.page === 1));
  const ids = hits.map((h) => h.chunk.id).sort();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('프리즘 문서 검색');
  await main.getByRole('option').filter({ hasText: 'scan-locked.pdf' }).waitFor();
  await main.screenshot({ path: join(evidence, 'ocr-korean-result.png') });
  // Idle UI before direct search IPC so revision-driven UI requests cannot
  // supersede the same window's explicit API request under test.
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('');
  dialog = await openOcrSettings();
  await dialog.getByRole('checkbox', { name: toggleName }).uncheck();
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await waitReady();
  assert.deepEqual(
    (await search()).hits.map((h) => h.chunk.id).sort(),
    ids,
    'disabling OCR retains already extracted text and IDs',
  );
  await copyFile('tests/fixtures/ocr/bilingual.png', join(folder, 'new-after-off.png'));
  await invoke('content_refresh');
  await waitReady();
  assert(!(await search()).hits.some((h) => h.chunk.filePath.endsWith('new-after-off.png')));
  await save((settings) => (settings.ocr = { enabled: true, language: 'zz-Unknown' }));
  await waitReady();
  assert((await invoke('content_status')).issues.some((i) => i.code === 'ocrLanguageUnavailable'));
  dialog = await openOcrSettings();
  await dialog
    .getByRole('status')
    .filter({ hasText: '선택한 OCR 언어가 설치되어 있지 않습니다.' })
    .waitFor();
  assert.equal(await dialog.getByLabel('OCR 언어', { exact: true }).inputValue(), 'zz-Unknown');
  await dialog.getByLabel('OCR 언어', { exact: true }).selectOption('ko');
  await dialog.getByRole('checkbox', { name: toggleName }).uncheck();
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await waitReady();
  console.log(JSON.stringify({ directory, phase: 'off-retained-and-language-diagnostic' }));
  // Explicitly import the actual pinned model; no fake or alternate vectors.
  await invoke('model_import', {
    path: resolve('.dev/models/embeddinggemma-2-Q8_0.gguf'),
    runtime: false,
  });
  await invoke('model_import', {
    path: resolve('.dev/runtime/llama-b11960-bin-win-cpu-x64.zip'),
    runtime: true,
  });
  await invoke('model_start');
  await until(
    async () => (await invoke('model_status')).phase === 'running',
    'actual pinned Q8 runtime',
    120000,
  );
  await invoke('content_refresh');
  await until(
    async () => {
      const s = await invoke('content_status');
      return s.phase === 'ready' && s.embedded === 5 && !s.embeddingError;
    },
    'OCR text embedded by actual Q8 into LanceDB',
    120000,
  );
  const semantic = await invoke('search_semantic', {
    query: '프리즘 로컬 문서 검색',
    extension: '',
    scopeId: null,
    requestId: crypto.randomUUID(),
  });
  assert(semantic.hits.some((h) => h.chunk.fingerprint.sha256 === sha));
  assert(
    semantic.hits.every(
      (h) =>
        !h.chunk.filePath.includes('filename-only') &&
        !h.chunk.filePath.endsWith('new-after-off.png'),
    ),
  );
  console.log(
    JSON.stringify({
      directory,
      phase: 'actual-q8-lance-search',
      semanticHits: semantic.hits.length,
    }),
  );
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory, options);
  ({ main, invoke } = instance);
  await waitReady();
  assert.equal((await invoke('get_settings')).ocr.enabled, false);
  assert.equal(
    (await invoke('model_status')).phase,
    'ready',
    'restart does not start the imported model',
  );
  assert.deepEqual((await search()).hits.map((h) => h.chunk.id).sort(), ids);
  await save((settings) => (settings.scopes[0].contentEnabled = false));
  await waitReady();
  assert.equal((await search()).hits.length, 0, 'scope revocation still hides cached OCR text');
  assert.deepEqual(await readFile(locked), original);
  const pdfFiles = (await readdir(directory, { recursive: true, withFileTypes: true })).filter(
    (e) => e.isFile() && /\.pdf$/i.test(e.name),
  );
  assert.equal(pdfFiles.length, 3, 'no decrypted PDF written');
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      engine: capabilities.engine,
      languages: capabilities.languages,
      model: 'actual pinned EmbeddingGemma 2 Q8',
      semanticHits: semantic.hits.length,
      sha,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
