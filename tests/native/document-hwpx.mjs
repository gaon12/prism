// Real immutable revisions, HWPX codec, atomic replacement and independent
// content extraction. Only the save picker response is supplied by the fixture;
// one real IPC is held to exercise cancellation before worker admission.
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readdir, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join, toNamespacedPath } from 'node:path';
import { expect } from '@playwright/test';
import { inspectHwpx } from './hwpx-inspect.mjs';
import { launch, shutdown, until } from './harness.mjs';
const directory = await mkdtemp(join(tmpdir(), 'prism-document-hwpx-'));
const pathKey = (value) =>
  process.platform === 'win32' ? toNamespacedPath(value).toLowerCase() : value;
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
  `**강조**와 *기울임* · \`literal code\` · \`#read(${JSON.stringify(secret)})\`\n\n` +
  `근거[^a], 다시[^a]\n\n[^a]: 사용자 작성 각주\n\n![이미지 설명](${imageUrl})\n\n` +
  '<script>literal HTML</script>\n\n| 항목 | 수치 |\n|---|---:|\n| 예산 | 120000원 |\n';
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
      title: '다국어 HWPX',
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
  const args = (revision, path, extra = {}) => ({
    id,
    revision,
    path,
    request: crypto.randomUUID(),
    includeSources: true,
    includeTypeMarkers: true,
    ...extra,
  });
  const oldPath = join(outputs, '과거 버전.hwpx');
  const oldResult = await invoke('doc_export_hwpx', args(old.id, oldPath));
  const archive = inspectHwpx(oldPath);
  assert.equal(oldResult.omittedImages, 1);
  assert.equal(oldResult.literalHtml, 1);
  assert(archive.text.includes('ARCHIVE_ONLY') && !archive.text.includes('LATEST_ONLY'));
  assert.equal((await invoke('doc_get', { id, revisionId: null })).revision.id, latest.id);
  const rejection = (page, packet) =>
    page.evaluate(async (packet) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_export_hwpx', packet);
        return null;
      } catch (error) {
        return error.code;
      }
    }, packet);
  assert.equal(await rejection(main, args(latest.id, join(outputs, 'wrong.pdf'))), 'input');
  const other = await invoke('doc_create', {
    input: {
      title: 'Other draft',
      purpose: 'separate revision',
      scopeIds: [],
      templateMarkdown: null,
    },
  });
  const foreignPath = join(outputs, 'foreign.hwpx');
  assert.equal(await rejection(main, args(other.revision.id, foreignPath)), 'notFound');
  await assert.rejects(stat(foreignPath));
  assert.equal(await rejection(quick, args(latest.id, join(outputs, 'quick.hwpx'))), 'input');
  const plainPath = join(outputs, '분류 제외.hwpx');
  await invoke('doc_export_hwpx', args(latest.id, plainPath, { includeTypeMarkers: false }));
  assert(!inspectHwpx(plainPath).text.includes('검증되지 않음'));
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__hwpxTest = { savePath: null, saveCalls: 0, exports: [], cancel: null, hold: false };
    window.__restoreHwpxFetch = () => {
      window.fetch = original;
      window.__releaseHwpx?.();
    };
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1)),
        test = window.__hwpxTest;
      if (command === 'plugin:dialog|save') {
        test.saveCalls++;
        return new Response(JSON.stringify(test.savePath), {
          headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
        });
      }
      if (command === 'doc_export_hwpx') {
        test.exports.push(JSON.parse(options.body));
        if (test.hold) {
          test.pending = true;
          await new Promise((resolve) => {
            window.__releaseHwpx = resolve;
          });
        }
      }
      const result = await original(input, options);
      if (command === 'doc_cancel_hwpx') test.cancel = await result.clone().json();
      return result;
    };
  });
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await expect(
    main.getByText('자료 선택 → 개요 생성·검토 → 본문 편집 → Markdown·PDF·HWPX 내보내기', {
      exact: true,
    }),
  ).toBeVisible();
  await main.locator('.doc-sidebar button').filter({ hasText: '다국어 HWPX' }).click();
  const editor = main.getByRole('region', { name: '다국어 HWPX', exact: true });
  await editor.locator('.doc-export > summary').click();
  const hwpxButton = editor.getByRole('button', { name: /^HWPX 내보내기/ });
  const pdfButton = editor.getByRole('button', { name: /^PDF 내보내기/ });
  await hwpxButton.click();
  await until(
    () => main.evaluate(() => window.__hwpxTest.saveCalls === 1),
    'cancelled save picker',
  );
  await expect(hwpxButton).toBeEnabled();
  assert.equal(await main.evaluate(() => window.__hwpxTest.exports.length), 0);
  await editor.getByRole('tab', { name: '본문 블록', exact: true }).click();
  const textarea = editor.locator('.doc-block-text').first();
  await textarea.fill(text('LATEST_ONLY') + '\nunsaved');
  await expect(hwpxButton).toBeDisabled();
  await expect(pdfButton).toBeDisabled();
  await textarea.fill(text('LATEST_ONLY'));
  await expect(hwpxButton).toBeEnabled();
  const cancelled = join(outputs, '취소.hwpx');
  await writeFile(cancelled, 'preserve existing selected file');
  await main.evaluate((path) => {
    window.__hwpxTest.savePath = path;
    window.__hwpxTest.hold = true;
    window.__hwpxEditor = document.querySelector('.doc-editor');
  }, cancelled);
  await hwpxButton.click();
  await until(() => main.evaluate(() => window.__hwpxTest.pending), 'held real export IPC');
  await expect(pdfButton).toBeDisabled();
  await editor.getByRole('button', { name: 'HWPX 내보내기 중단', exact: true }).click();
  await until(
    () => main.evaluate(() => window.__hwpxTest.cancel === true),
    'accepted early cancel',
  );
  assert(await main.evaluate(() => window.__hwpxEditor === document.querySelector('.doc-editor')));
  await main.evaluate(() => {
    window.__hwpxTest.hold = false;
    window.__releaseHwpx();
  });
  await expect(
    editor.getByText('HWPX 내보내기를 중단했습니다. 기존 파일은 그대로입니다.', { exact: true }),
  ).toBeVisible();
  assert.equal(await readFile(cancelled, 'utf8'), 'preserve existing selected file');
  const finalPath = join(outputs, '최신 보고서.hwpx');
  await main.evaluate((path) => {
    window.__hwpxTest.savePath = path;
  }, finalPath);
  await hwpxButton.click();
  await expect(
    editor.getByText('HWPX로 저장했습니다. 이미지·HTML은 대체 문구나 텍스트로 표시했습니다.', {
      exact: true,
    }),
  ).toBeVisible();
  const result = inspectHwpx(finalPath);
  for (const phrase of [
    '한국어',
    '中文',
    '日本語',
    'English',
    '😀',
    '74829',
    'LATEST_ONLY',
    '사용자 작성 각주',
    'literal code',
    'literal HTML',
    '검증되지 않음',
    '120000원',
  ])
    assert(result.text.includes(phrase), `Missing HWPX content: ${phrase}`);
  assert(
    !result.text.includes('ARCHIVE_ONLY') &&
      !result.text.includes('PRIVATE_FILE_MUST_NOT_BE_READ_72914'),
  );
  assert(!result.text.includes('`literal code`'));
  assert.equal(result.tables, 1);
  assert(result.notes >= 1);
  assert.equal(result.images, 0);
  assert.equal(network, 0);
  assert.deepEqual((await invoke('get_settings')).scopes, settingsBefore.scopes);
  const original = await readFile(finalPath);
  // Grant this private fixture folder content indexing only after export; the
  // export itself must not grant collection, file or external-AI permissions.
  const before = await invoke('get_settings');
  await invoke('save_settings', {
    expectedRevision: before.revision,
    settings: {
      ...before,
      scopes: [
        {
          id: 'generated',
          name: 'HWPX fixture',
          path: outputs,
          filenameEnabled: true,
          contentEnabled: true,
          externalAllowed: false,
        },
      ],
    },
  });
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed >= 3;
  }, 'independent HWPX extraction');
  for (const query of ['한국어', '中文', '日本語', 'English', '120000', '사용자 작성 각주']) {
    const found = await invoke('search_content', {
      query,
      extension: 'hwpx',
      scopeId: 'generated',
      requestId: crypto.randomUUID(),
    });
    assert(
      found.hits.some((hit) => pathKey(hit.chunk.filePath) === pathKey(finalPath)),
      `Independent extraction missing ${query}`,
    );
  }
  assert.deepEqual(await readFile(finalPath), original);
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  await editor
    .locator('.doc-export')
    .evaluate((element) => element.scrollIntoView({ block: 'end' }));
  await expect(hwpxButton).toBeInViewport();
  await main.screenshot({ path: join(directory, 'export-ko-light.png') });
  const current = await invoke('get_settings');
  await invoke('save_settings', {
    expectedRevision: current.revision,
    settings: { ...current, language: 'en', theme: 'dark' },
  });
  await main.setViewportSize({ width: 760, height: 760 });
  const englishButton = editor.getByRole('button', { name: /^Export HWPX/ });
  await editor
    .locator('.doc-export')
    .evaluate((element) => element.scrollIntoView({ block: 'end' }));
  await expect(englishButton).toBeInViewport();
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await main.screenshot({ path: join(directory, 'export-en-dark760.png') });
  assert(!(await readdir(outputs)).some((name) => name.startsWith('.prism-export-')));
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      directory,
      parts: result.parts,
      tables: result.tables,
      notes: result.notes,
      oldRevision: true,
      earlyCancel: true,
      sourcePreserved: true,
      network,
      errors: 0,
    }),
  );
} finally {
  if (instance) {
    await instance.main.evaluate(() => window.__restoreHwpxFetch?.()).catch(() => {});
    await shutdown(instance);
  }
  await new Promise((resolve) => tracker.close(resolve));
}
