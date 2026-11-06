// Real saved revisions, portable typesetter, atomic file writer and native PDF
// rasterization. Only the OS save picker is replaced; one real IPC request is
// held to exercise early cancellation. No PDF/backend result is fabricated.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readdir, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect } from '@playwright/test';
import { inspectPdf } from './pdf-inspect.mjs';
import { launch, shutdown, until } from './harness.mjs';
const directory = await mkdtemp(join(tmpdir(), 'prism-document-pdf-'));
const profile = join(directory, 'profile'),
  outputs = join(directory, 'exports');
await mkdir(profile);
await mkdir(outputs);
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [],
  }),
);
const secret = join(directory, 'private.txt');
await writeFile(secret, 'PRIVATE_FILE_MUST_NOT_BE_READ_72914');
let network = 0;
const tracker = createServer((_, response) => {
  network++;
  response.end('not used');
});
await new Promise((resolve) => tracker.listen(0, '127.0.0.1', resolve));
const imageUrl = `http://127.0.0.1:${tracker.address().port}/private.png`;
const text = (marker) =>
  `## 본문 자료\n\n한국어 中文 日本語 English 😀 74829 ${marker}\n\n` +
  `**강조**와 *기울임* · \`#read(${JSON.stringify(secret)})\`\n\n` +
  `근거[^a], 다시[^a]\n\n[^a]: 사용자 작성 각주\n\n![이미지 설명](${imageUrl})\n\n` +
  '| 반복 머리글 | 수치 |\n|---|---:|\n' +
  Array.from({ length: 130 }, (_, i) => `| 자료 ${i} 한글 中文 日本語 | ${i} |`).join('\n');
const block = (markdown) => ({
  id: 'report',
  sectionId: null,
  kind: 'userInput',
  markdown,
  sourceLabels: [],
  claimsVerified: false,
  userEdited: true,
});
let instance;
try {
  instance = await launch(
    profile,
    process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
  );
  const { main, quick, invoke, errors } = instance;
  const settingsBefore = await invoke('get_settings');
  const created = await invoke('doc_create', {
    input: {
      title: '다국어 보고서',
      purpose: 'manual fixture',
      scopeIds: [],
      templateMarkdown: null,
    },
  });
  const id = created.document.id;
  const old = await invoke('doc_save_manual', {
    id,
    expected: created.revision.id,
    blocks: [block(text('ARCHIVE_ONLY'))],
    plan: null,
  });
  const latest = await invoke('doc_save_manual', {
    id,
    expected: old.id,
    blocks: [block(text('LATEST_ONLY'))],
    plan: null,
  });
  const exportArgs = (revision, path, extra = {}) => ({
    id,
    revision,
    path,
    request: crypto.randomUUID(),
    includeSources: true,
    includeTypeMarkers: true,
    ...extra,
  });
  const oldPath = join(outputs, '과거 버전.pdf');
  const oldResult = await invoke('doc_export_pdf', exportArgs(old.id, oldPath));
  const archive = inspectPdf(oldPath);
  assert.equal(archive.pages, oldResult.pages);
  assert(archive.pages >= 3);
  assert(archive.text.includes('ARCHIVE_ONLY') && !archive.text.includes('LATEST_ONLY'));
  assert.equal((await invoke('doc_get', { id, revisionId: null })).revision.id, latest.id);
  // Invalid paths and stale/foreign revision identifiers cannot create output.
  const rejected = await main.evaluate(
    async (packet) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_export_pdf', packet);
        return null;
      } catch (error) {
        return error.code;
      }
    },
    exportArgs(latest.id, join(outputs, 'wrong.md')),
  );
  assert.equal(rejected, 'input');
  const other = await invoke('doc_create', {
    input: {
      title: 'Other draft',
      purpose: 'separate revision',
      scopeIds: [],
      templateMarkdown: null,
    },
  });
  const foreignPath = join(outputs, 'foreign.pdf');
  const foreignRejected = await main.evaluate(
    async (packet) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_export_pdf', packet);
        return null;
      } catch (error) {
        return error.code;
      }
    },
    exportArgs(other.revision.id, foreignPath),
  );
  assert.equal(foreignRejected, 'notFound');
  await assert.rejects(stat(foreignPath));
  const quickRejected = await quick.evaluate(
    async (packet) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_export_pdf', packet);
        return null;
      } catch (error) {
        return error.code;
      }
    },
    exportArgs(latest.id, join(outputs, 'quick.pdf')),
  );
  assert.equal(quickRejected, 'input');
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__restorePdfFetch = () => {
      window.fetch = original;
      window.__releasePdf?.();
    };
    window.__pdfTest = { savePath: null, saveCalls: 0, exports: [], cancel: null, hold: false };
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1)),
        test = window.__pdfTest;
      if (command === 'plugin:dialog|save') {
        test.saveCalls++;
        return new Response(JSON.stringify(test.savePath), {
          headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
        });
      }
      if (command === 'doc_export_pdf') {
        test.exports.push(JSON.parse(options.body));
        if (test.hold) {
          test.pending = true;
          await new Promise((resolve) => {
            window.__releasePdf = resolve;
          });
        }
      }
      const result = await original(input, options);
      if (command === 'doc_cancel_pdf') test.cancel = await result.clone().json();
      return result;
    };
  });
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main.locator('.doc-sidebar button').filter({ hasText: '다국어 보고서' }).click();
  const editor = main.getByRole('region', { name: '다국어 보고서', exact: true });
  await editor.locator('.doc-export > summary').click();
  const pdfButton = editor.getByRole('button', { name: /^PDF 내보내기/ });
  await pdfButton.click();
  await until(
    async () => await main.evaluate(() => window.__pdfTest.saveCalls === 1),
    'cancelled native save picker',
  );
  await expect(pdfButton).toBeEnabled();
  assert.equal(await main.evaluate(() => window.__pdfTest.exports.length), 0);
  // Actual editor changes must be saved before either export button enables.
  await editor.getByRole('tab', { name: '본문 블록', exact: true }).click();
  const textarea = editor.locator('.doc-block-text').first();
  await textarea.fill(text('LATEST_ONLY') + '\nunsaved');
  await expect(pdfButton).toBeDisabled();
  await expect(editor.getByText('변경사항을 저장한 뒤 내보내세요.', { exact: true })).toBeVisible();
  await textarea.fill(text('LATEST_ONLY'));
  await expect(pdfButton).toBeEnabled();
  const cancelled = join(outputs, '취소.pdf');
  await main.evaluate((path) => {
    window.__pdfTest.savePath = path;
    window.__pdfTest.hold = true;
    window.__pdfEditor = document.querySelector('.doc-editor');
  }, cancelled);
  await pdfButton.click();
  await until(
    () => main.evaluate(() => window.__pdfTest.pending),
    'real export request held before admission',
  );
  await editor.getByRole('button', { name: 'PDF 내보내기 중단', exact: true }).click();
  await until(
    () => main.evaluate(() => window.__pdfTest.cancel === true),
    'real early cancellation accepted',
  );
  assert(await main.evaluate(() => window.__pdfEditor === document.querySelector('.doc-editor')));
  await main.evaluate(() => {
    window.__pdfTest.hold = false;
    window.__releasePdf();
  });
  await expect(
    editor.getByText('PDF 내보내기를 중단했습니다. 기존 파일은 그대로입니다.', { exact: true }),
  ).toBeVisible();
  await assert.rejects(stat(cancelled));
  const finalPath = join(outputs, '최신 보고서.pdf');
  await main.evaluate((path) => {
    window.__pdfTest.savePath = path;
  }, finalPath);
  await pdfButton.click();
  await expect(
    editor.getByText('PDF로 저장했습니다. 이미지는 대체 문구로 표시했습니다.', { exact: true }),
  ).toBeVisible();
  const result = inspectPdf(finalPath);
  assert(result.pages >= 3 && !result.encrypted);
  for (const phrase of [
    '한국어',
    '中文',
    '日本語',
    'English',
    '😀',
    '74829',
    'LATEST_ONLY',
    '사용자 작성 각주',
  ])
    assert(result.text.includes(phrase), `Missing native PDF content: ${phrase}`);
  assert(
    !result.text.includes('ARCHIVE_ONLY') &&
      !result.text.includes('PRIVATE_FILE_MUST_NOT_BE_READ_72914'),
  );
  assert(result.text.includes('검증되지 않음'));
  for (const [width, height] of result.sizes) {
    assert(Math.abs(width - 595.28) < 1);
    assert(Math.abs(height - 841.89) < 1);
  }
  assert.equal(network, 0);
  // Reimport only grants collection viewing, not content/AI indexing.
  const collection = await invoke('collection_create', { title: 'Export check', description: '' });
  const work = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: 'PDF',
    metadata: {
      kind: 'document',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  let unit = await invoke('collection_unit_create', {
    workId: work.id,
    title: 'Pages',
    kind: 'document',
  });
  unit = await invoke('collection_import', {
    unitId: unit.id,
    expected: unit.revision,
    paths: [finalPath],
  });
  const pages = (
    await invoke('collection_pages', {
      unitId: unit.id,
      expected: unit.revision,
      offset: 0,
      limit: 1,
    })
  ).items;
  const media = await invoke('collection_media', { pageId: pages[0].id, thumbnail: false });
  assert(media.dataUrl.startsWith('data:image/webp;base64,'));
  await writeFile(
    join(directory, 'actual-pdf-first-page.webp'),
    Buffer.from(media.dataUrl.split(',')[1], 'base64'),
  );
  assert.equal(unit.pageCount, result.pages);
  const ink = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    let ink = 0;
    const pixels = context.getImageData(0, 0, image.width, image.height).data;
    for (let i = 0; i < pixels.length; i += 4)
      if (pixels[i] < 180 && pixels[i + 1] < 180 && pixels[i + 2] < 180) ink++;
    return ink;
  }, media.dataUrl);
  assert(ink > 2000);
  await editor.locator('.doc-export').scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(directory, 'export-ko-light.png') });
  const before = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...before, language: 'en', theme: 'dark' },
    expectedRevision: before.revision,
  });
  await main.setViewportSize({ width: 760, height: 760 });
  const englishPdfButton = editor.getByRole('button', { name: /^Export PDF/ });
  await expect(englishPdfButton).toBeVisible();
  await editor
    .locator('.doc-export')
    .evaluate((element) => element.scrollIntoView({ block: 'end' }));
  await expect(englishPdfButton).toBeInViewport();
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await main.screenshot({ path: join(directory, 'export-en-dark760.png') });
  assert.deepEqual((await invoke('get_settings')).scopes, settingsBefore.scopes);
  assert.equal((await invoke('content_status')).indexed, 0);
  assert(!(await readdir(outputs)).some((name) => name.startsWith('.prism-export-')));
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      directory,
      pages: result.pages,
      actualInk: ink,
      earlyCancel: true,
      oldRevision: true,
      network,
      errors: 0,
    }),
  );
} finally {
  if (instance) {
    await instance.main.evaluate(() => window.__restorePdfFetch?.()).catch(() => {});
    await shutdown(instance);
  }
  await new Promise((resolve) => tracker.close(resolve));
}
