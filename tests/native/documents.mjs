// Actual document/RAG/source/storage services. Generation is a synthetic local
// server; documents, revisions and source files are all isolated fixtures.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { createServer } from 'node:http';
import { copyFile, mkdir, mkdtemp, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, toNamespacedPath } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';
import { inspectPdf } from './pdf-inspect.mjs';
import { inspectHwpx } from './hwpx-inspect.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-documents-'));
const evidence = join(directory, 'evidence');
await mkdir(evidence, { recursive: true });
const portable = Boolean(process.env.PRISM_TEST_EXECUTABLE);
const application = join(directory, 'app');
const profile = portable ? join(application, 'PrismData') : directory;
await mkdir(profile, { recursive: true });
const scopeId = `document_fixture_${randomUUID()}`;
const pathKey = (value) =>
  process.platform === 'win32' ? toNamespacedPath(value).toLowerCase() : value;
const sha = async (path) => {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
};
let executable, executableSha256;
if (portable) {
  assert.equal(process.platform, 'win32', 'This portable native fixture uses Windows WebView2');
  const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
  assert((await stat(original)).isFile());
  executableSha256 = await sha(original);
  if (process.env.PRISM_TEST_APP_SHA256)
    assert.equal(executableSha256, process.env.PRISM_TEST_APP_SHA256.toLowerCase());
  executable = join(application, 'prism.exe');
  await copyFile(original, executable);
  await writeFile(join(application, 'prism.portable'), '');
  assert.equal(await sha(executable), executableSha256);
}
const launchOptions = portable ? { executable, portable: true, productionPortable: true } : {};
const folder = join(directory, 'source-documents');
await mkdir(folder);
const source = join(folder, '원본.md');
await writeFile(source, 'shared 제주 출장 경비 120000원. 표와 일정 검토.');
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    shortcut: 'Ctrl+Alt+Shift+F6',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: scopeId,
        name: '문서 근거',
        path: folder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
const calls = [],
  sockets = new Set();
let behavior = 'normal';
let releaseUiBody;
const literalMarkdown =
  '모델 분류 예시\n\n`CODE_LITERAL_S1 [S1]` · `CODE_LITERAL_FOOTNOTE [^S1]`\n\n' +
  '```text\nFENCED_LITERAL [S1]\n```\n\n' +
  '[링크](https://example.invalid/[S1])\n\nEscaped \\[S1]\n\n' +
  'Authored note[^s1].\n\n[^S1]: AUTHOR_NOTE_CANARY\n\n' +
  '````text\nUNFINISHED_LITERAL [S1]\n```';
const server = createServer((request, response) => {
  const buffers = [];
  request.on('data', (chunk) => buffers.push(chunk));
  request.on('end', () => {
    const packet = JSON.parse(Buffer.concat(buffers).toString());
    const current = packet.messages.at(-1).content;
    const marker = 'Document input:\n';
    const input = JSON.parse(
      current.slice(current.lastIndexOf(marker) + marker.length).split('\n')[0],
    );
    calls.push({ packet, input });
    if (behavior === 'stall') return;
    const payload =
      input.operation === 'outline'
        ? {
            title: 'shared',
            sections: [{ id: 'summary', heading: '경비와 검토', instruction: 'shared' }],
          }
        : input.operation === 'rewrite'
          ? {
              blocks: [
                {
                  id: input.targetBlockIds[0],
                  sectionId: 'summary',
                  kind: 'aiProposal',
                  markdown: input.selection ? '수정' : '다듬은 제안',
                  sourceLabels: [],
                },
              ],
            }
          : {
              blocks: [
                {
                  id: 'facts',
                  sectionId: 'summary',
                  kind: 'fact',
                  markdown: '| 항목 | 경비 |\n|---|---:|\n| 제주도 | 120000원 |\n\n[S1]',
                  sourceLabels: ['[S1]'],
                },
                {
                  id: 'proposal',
                  sectionId: 'summary',
                  kind: 'aiProposal',
                  markdown: 'A🙂한글Z',
                  sourceLabels: [],
                },
                {
                  id: 'blank',
                  sectionId: 'summary',
                  kind: 'blank',
                  markdown: '',
                  sourceLabels: [],
                },
                {
                  id: 'classified',
                  sectionId: 'summary',
                  kind: 'userInput',
                  markdown: literalMarkdown,
                  sourceLabels: ['[S1]'],
                },
              ],
            };
    const text = behavior === 'malformed' ? 'synthetic invalid JSON' : JSON.stringify(payload);
    const body = JSON.stringify({
      model: 'synthetic-document-generator',
      choices: [{ message: { content: text }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 11, completion_tokens: 9, total_tokens: 20 },
    });
    response.writeHead(200, {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(body),
    });
    if (behavior === 'uiBodyGate' && input.operation === 'body')
      releaseUiBody = () => response.end(body);
    else response.end(body);
  });
});
server.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
let instance;
let verifiedProfile = false;
let runtime, surface;
const warnings = [];
async function verifyProfile(current) {
  const settings = await current.invoke('get_settings');
  assert.equal(settings.scopes.length, 1);
  assert.equal(settings.scopes[0].id, scopeId, 'CDP must reach the fresh fixture profile');
  assert.equal(pathKey(settings.scopes[0].path), pathKey(folder));
  const info = await current.invoke('runtime_info');
  if (portable) assert.equal(info.installKind, 'portable');
  if (process.env.PRISM_TEST_EXPECT_RELEASE === '1') assert.equal(info.development, false);
  verifiedProfile = true;
  for (const page of [current.main, current.quick])
    page.on('console', (entry) => {
      if (entry.type() === 'warning') warnings.push(entry.text());
    });
  return info;
}
try {
  instance = await launch(profile, launchOptions);
  runtime = await verifyProfile(instance);
  const { invoke, main } = instance;
  assert.equal((await invoke('doc_list', { query: '', limit: 100 })).length, 0);
  await main.setViewportSize({ width: 1280, height: 840 });
  surface = {
    url: main.url(),
    title: await main.title(),
    viewport: main.viewportSize(),
    browserVersion: await instance.browser.version(),
  };
  assert.equal(await main.locator('vite-error-overlay').count(), 0);
  await main.screenshot({ path: join(evidence, 'documents-initial-native.png') });
  const result = (command, args) =>
    main.evaluate(
      async ({ command, args }) => {
        try {
          return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(command, args) };
        } catch (error) {
          return { ok: false, error: { code: error?.code, rag: error?.rag } };
        }
      },
      { command, args },
    );
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 1;
  }, 'document source indexed');
  const settings = await invoke('get_settings');
  await invoke('save_settings', {
    expectedRevision: settings.revision,
    settings: {
      ...settings,
      generation: {
        ...settings.generation,
        maxContextChars: 40000,
        local: {
          endpoint: `http://127.0.0.1:${server.address().port}/v1`,
          model: 'synthetic-document-generator',
          maxTokens: 2048,
          tokenParameter: 'maxTokens',
          temperature: null,
        },
      },
    },
  });
  const created = await invoke('doc_create', {
    input: { title: 'shared', purpose: 'shared', scopeIds: [scopeId], templateMarkdown: null },
  });
  const id = created.document.id;
  const request = (revision, scopeIds = [scopeId]) => ({
    documentId: id,
    expectedRevisionId: revision.id,
    requestId: crypto.randomUUID(),
    instruction: 'shared',
    scopeIds,
  });
  const early = await result('doc_generate_body', { request: request(created.revision) });
  assert.equal(early.error.code, 'outlineReviewRequired');
  assert.equal(calls.length, 0);
  const noScopes = await result('doc_generate_outline', { request: request(created.revision, []) });
  assert.equal(noScopes.error.code, 'input');
  assert.equal(calls.length, 0);
  const outline = await invoke('doc_generate_outline', { request: request(created.revision) });
  assert.equal(outline.outlineReviewed, false);
  const reviewed = await invoke('doc_save_outline', {
    id,
    expected: outline.id,
    plan: outline.plan,
    reviewed: true,
  });
  const body = await invoke('doc_generate_body', { request: request(reviewed) });
  assert.equal(body.claimsVerified, false);
  assert(body.blocks.every((block) => !block.claimsVerified && !block.userEdited));
  assert(body.markdown.includes('120000원'));
  assert(body.blocks.some((block) => block.kind === 'blank' && block.markdown === ''));
  assert.equal(
    (await invoke('rag_list', { query: '', limit: 100 })).length,
    0,
    'Document generation history must stay out of chat lists',
  );
  assert.equal((await invoke('rag_list', { query: '120000', limit: 100 })).length, 0);
  // The normal provider response has four valid blocks assigned to summary.
  // A second reviewed section must not disappear just because JSON is valid.
  const coverage = await invoke('doc_create', {
    input: {
      title: 'Incomplete body guard',
      purpose: 'shared',
      scopeIds: [scopeId],
      templateMarkdown: null,
    },
  });
  const coverageId = coverage.document.id;
  const coverageReviewed = await invoke('doc_save_outline', {
    id: coverageId,
    expected: coverage.revision.id,
    plan: {
      title: 'Incomplete body guard',
      sections: [
        { id: 'summary', heading: 'Summary', instruction: 'shared' },
        { id: 'details', heading: 'Details', instruction: 'shared details' },
      ],
    },
    reviewed: true,
  });
  const coverageSaved = await invoke('doc_save_manual', {
    id: coverageId,
    expected: coverageReviewed.id,
    blocks: ['summary', 'details'].map((sectionId) => ({
      id: `saved_${sectionId}`,
      sectionId,
      kind: 'userInput',
      markdown: `Existing ${sectionId}`,
      sourceLabels: [],
      claimsVerified: false,
      userEdited: true,
    })),
    plan: null,
  });
  const coverageHistory = await invoke('doc_revisions', {
    id: coverageId,
    before: null,
    limit: 100,
  });
  const coverageCalls = calls.length;
  const incomplete = await result('doc_generate_body', {
    request: {
      ...request(coverageSaved),
      documentId: coverageId,
    },
  });
  assert.equal(
    incomplete.error?.code,
    'malformed',
    'Body generation must not silently omit a reviewed section',
  );
  assert.deepEqual(
    (await invoke('doc_get', { id: coverageId, revisionId: null })).revision,
    coverageSaved,
  );
  assert.deepEqual(
    await invoke('doc_revisions', { id: coverageId, before: null, limit: 100 }),
    coverageHistory,
  );
  assert.equal((await invoke('doc_job', { id: coverageId })).phase, 'failed');
  assert.equal(calls.length, coverageCalls + 1, 'Incomplete bodies are not repaired or retried');
  await invoke('doc_delete', { id: coverageId });
  const modified = structuredClone(body.blocks);
  modified.find((block) => block.id === 'blank').kind = 'userInput';
  modified.find((block) => block.id === 'blank').markdown = '사용자가 채운 값';
  const manual = await invoke('doc_save_manual', {
    id,
    expected: body.id,
    blocks: modified,
    plan: null,
  });
  assert.equal(
    manual.blocks.find((block) => block.id === 'facts').userEdited,
    false,
    'Untouched model blocks must not become user edits',
  );
  assert.equal(manual.blocks.find((block) => block.id === 'blank').userEdited, true);
  const stale = await result('doc_save_manual', {
    id,
    expected: body.id,
    blocks: modified,
    plan: null,
  });
  assert.equal(stale.error.code, 'conflict');
  const rewritten = await invoke('doc_rewrite', {
    request: {
      ...request(manual),
      targetBlockIds: ['proposal'],
      selection: { blockId: 'proposal', start: 1, end: 3 },
    },
  });
  assert.equal(rewritten.blocks.find((block) => block.id === 'proposal').markdown, 'A수정글Z');
  assert.equal(
    rewritten.blocks.find((block) => block.id === 'facts').markdown,
    manual.blocks.find((block) => block.id === 'facts').markdown,
  );
  const larger = structuredClone(rewritten.blocks);
  larger.find((block) => block.id === 'proposal').markdown += ' 긴 문서 내용'.repeat(600);
  const longRevision = await invoke('doc_save_manual', {
    id,
    expected: rewritten.id,
    blocks: larger,
    plan: null,
  });
  const longRewrite = await invoke('doc_rewrite', {
    request: { ...request(longRevision), targetBlockIds: ['proposal'], selection: null },
  });
  assert.equal(longRewrite.blocks.find((block) => block.id === 'proposal').markdown, '다듬은 제안');
  assert(
    Buffer.byteLength(calls.at(-1).packet.messages.at(-1).content) > 4096,
    'Long derived input must reach generation without becoming the retrieval query',
  );
  behavior = 'malformed';
  const malformed = await result('doc_generate_body', { request: request(longRewrite) });
  assert.equal(malformed.error.code, 'malformed');
  assert.equal((await invoke('doc_get', { id, revisionId: null })).revision.id, longRewrite.id);
  behavior = 'stall';
  const pendingRequest = request(longRewrite),
    before = calls.length;
  const pending = result('doc_generate_body', { request: pendingRequest });
  await until(() => calls.length > before, 'document provider starts stalled operation');
  await invoke('doc_cancel', { id, request: pendingRequest.requestId });
  assert.equal((await pending).error.code, 'cancelled');
  assert.equal((await invoke('doc_get', { id, revisionId: null })).revision.id, longRewrite.id);
  behavior = 'normal';
  const restored = await invoke('doc_restore', { id, expected: longRewrite.id, target: body.id });
  assert(restored.sequence > longRewrite.sequence);
  assert.equal(restored.markdown, body.markdown);
  assert((await invoke('doc_revisions', { id, before: null, limit: 100 })).length >= 8);
  const exported = await invoke('doc_export', {
    id,
    revision: null,
    includeSources: true,
    includeTypeMarkers: true,
  });
  assert(
    exported.includes('120000원') && exported.includes('prism:fact') && exported.includes('[^S1]'),
  );
  assert(exported.includes(literalMarkdown), 'Code, escapes and link targets must remain literal');
  assert(calls.every(({ packet }) => !JSON.stringify(packet).includes(directory)));

  // PDF uses this same real saved revision and its authenticated RAG citation
  // metadata. Exporting neither generates new content nor calls the provider.
  const pdfDirectory = await mkdtemp(join(tmpdir(), 'prism-document-source-pdf-'));
  const beforePdf = calls.length;
  for (const sources of [true, false]) {
    const path = join(pdfDirectory, sources ? 'with-sources.pdf' : 'without-sources.pdf');
    const result = await invoke('doc_export_pdf', {
      id,
      revision: restored.id,
      path,
      request: crypto.randomUUID(),
      includeSources: sources,
      includeTypeMarkers: sources,
    });
    const inspected = inspectPdf(path);
    assert.equal(result.pages, inspected.pages);
    assert(inspected.text.includes('120000원'));
    assert(inspected.text.includes('AUTHOR_NOTE_CANARY'), 'PDF must retain the authored note');
    for (const literal of [
      'CODE_LITERAL_S1[S1]',
      'CODE_LITERAL_FOOTNOTE[^S1]',
      'FENCED_LITERAL[S1]',
      'UNFINISHED_LITERAL[S1]',
    ])
      assert(inspected.text.replace(/\s+/g, '').includes(literal), `PDF literal: ${literal}`);
    assert.equal(
      inspected.text.includes('원본.md'),
      sources,
      'PDF source footnotes follow the saved-revision option',
    );
    assert.equal(inspected.text.replace(/\s+/g, '').includes('검증되지않음'), sources);
    const hwpxPath = join(pdfDirectory, sources ? 'with-sources.hwpx' : 'without-sources.hwpx');
    await invoke('doc_export_hwpx', {
      id,
      revision: restored.id,
      path: hwpxPath,
      request: crypto.randomUUID(),
      includeSources: sources,
      includeTypeMarkers: sources,
    });
    const hwpx = inspectHwpx(hwpxPath);
    assert(hwpx.text.includes('120000원'));
    for (const literal of [
      'CODE_LITERAL_S1[S1]',
      'CODE_LITERAL_FOOTNOTE[^S1]',
      'FENCED_LITERAL[S1]',
      'UNFINISHED_LITERAL[S1]',
    ])
      assert(hwpx.text.replace(/\s+/g, '').includes(literal), `HWPX literal: ${literal}`);
    assert.equal(hwpx.notes, sources ? 3 : 1, 'Authored notes survive the source option');
    assert.equal(
      hwpx.noteTexts.filter((text) => text.includes('AUTHOR_NOTE_CANARY')).length,
      1,
      'The authored reference must keep its own note content',
    );
    assert.equal(
      hwpx.noteTexts.filter((text) => text.includes('원본.md')).length,
      sources ? 2 : 0,
      'Only source references may receive source-file note content',
    );
    assert.equal(
      hwpx.text.includes('원본.md'),
      sources,
      'HWPX source footnotes follow the saved-revision option',
    );
    assert.equal(hwpx.text.replace(/\s+/g, '').includes('검증되지않음'), sources);
  }
  assert(exported.includes('[^prism-source-S1]:'));
  assert(exported.includes('[^S1]: AUTHOR_NOTE_CANARY'));
  assert.equal(calls.length, beforePdf);

  // Rendered UI calls the same real IPC. Creating/reviewing/saving remains explicit.
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main.getByRole('button', { name: '새 문서', exact: true }).click();
  await main.getByLabel('문서 제목', { exact: true }).fill('화면 검증');
  await main.getByLabel('작성 목적·문서 유형', { exact: true }).fill('shared');
  await main.getByRole('button', { name: '검색', exact: true }).click();
  assert.equal(
    await main.getByLabel('작성 목적·문서 유형', { exact: true }).inputValue(),
    'shared',
  );
  const beforeCreate = calls.length;
  await main.getByRole('button', { name: '초안 만들기', exact: true }).click();
  await main.locator('.doc-editor h2').filter({ hasText: '화면 검증' }).waitFor();
  assert.equal(calls.length, beforeCreate, 'Creating a draft must not call generation');
  const uiDoc = (await invoke('doc_list', { query: '화면 검증', limit: 100 }))[0];
  const outlineButton = main.getByRole('button', { name: 'AI로 개요 생성', exact: true });
  await main.locator('.doc-options > summary').click();
  const scopeBox = main.locator('.doc-options .doc-scopes input[type=checkbox]');
  await scopeBox.uncheck();
  assert.equal(await outlineButton.isDisabled(), true);
  await scopeBox.check();
  await main.locator('.doc-options > summary').click();
  await outlineButton.click();
  await main.getByLabel('항목 제목', { exact: true }).waitFor();
  await main.getByRole('tab', { name: '본문 블록', exact: true }).click();
  const bodyButton = main.getByRole('button', { name: '검토한 개요로 본문 생성', exact: true });
  assert.equal(await bodyButton.isDisabled(), true, 'Review must not be inferred from AI output');
  await main.getByRole('tab', { name: '개요', exact: true }).click();
  await main.getByLabel('개요를 검토했습니다.', { exact: true }).check();
  await main.getByRole('button', { name: '검토한 개요 저장', exact: true }).click();
  await until(
    async () => (await bodyButton.isVisible()) && !(await bodyButton.isDisabled()),
    'reviewed body action',
  );
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__heldActiveDocJob = null;
    window.__pauseFutureDocJobs = false;
    window.__futureDocJobReleases = [];
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      const isJob = url.hostname === 'ipc.localhost' && url.pathname.endsWith('/doc_job');
      if (isJob && window.__pauseFutureDocJobs)
        await new Promise((resolve) => window.__futureDocJobReleases.push(resolve));
      const response = await original(input, options);
      if (isJob && !window.__heldActiveDocJob) {
        const status = await response.clone().json();
        if (status && ['preparing', 'body'].includes(status.phase)) {
          window.__heldActiveDocJob = status;
          window.__pauseFutureDocJobs = true;
          await new Promise((resolve) => {
            window.__releaseStaleDocJob = resolve;
          });
        }
      }
      return response;
    };
  });
  behavior = 'uiBodyGate';
  await bodyButton.click();
  await until(() => Boolean(releaseUiBody), 'actual loopback body response pending');
  await until(
    () => main.evaluate(() => Boolean(window.__heldActiveDocJob)),
    'actual active document status held',
  );
  behavior = 'normal';
  releaseUiBody();
  await until(
    async () => (await main.locator('.doc-block').count()) === 4,
    'four generated body blocks',
  );
  assert.equal(await main.locator('.doc-user-edited').count(), 0);
  const proposal = main.locator('.doc-block').nth(1).getByLabel('블록 Markdown', { exact: true });
  await proposal.focus();
  await proposal.press('Home');
  await proposal.press('ArrowRight');
  await proposal.press('Shift+ArrowRight');
  await proposal.press('Shift+ArrowRight'); // Select the emoji and Korean character as real input.
  await main.evaluate(async () => {
    window.__releaseStaleDocJob();
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  assert(
    await main.getByRole('button', { name: '텍스트 선택 영역 다시 쓰기', exact: true }).isEnabled(),
    'An older active-status read cannot disable a completed document',
  );
  await main.getByRole('button', { name: '텍스트 선택 영역 다시 쓰기', exact: true }).click();
  await until(
    async () => (await proposal.inputValue()) === 'A수정글Z',
    'Unicode selection rewrite',
  );
  await main.evaluate(() => {
    window.__pauseFutureDocJobs = false;
    for (const release of window.__futureDocJobReleases) release();
    window.__futureDocJobReleases = [];
  });
  const blank = main.locator('.doc-block').nth(2);
  await blank.getByLabel('블록 분류', { exact: true }).selectOption('userInput');
  await blank.getByLabel('블록 Markdown', { exact: true }).fill('화면에서 채운 값');
  assert.equal(
    await main.getByRole('button', { name: '기존 로컬 메모 열기', exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await main.getByRole('button', { name: '기존 양식 채우기', exact: true }).isDisabled(),
    true,
    'switching to form filling must not discard an unsaved generated-document edit',
  );
  await main.getByRole('button', { name: '채팅', exact: true }).click();
  assert.equal(
    await blank.getByLabel('블록 Markdown', { exact: true }).inputValue(),
    '화면에서 채운 값',
  );
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__manualSaveHeld = false;
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (
        url.hostname === 'ipc.localhost' &&
        url.pathname.endsWith('/doc_save_manual') &&
        !window.__manualSaveHeld
      ) {
        window.__manualSaveHeld = true;
        await new Promise((resolve) => {
          window.__releaseManualSave = resolve;
        });
      }
      return original(input, options);
    };
  });
  await main.getByRole('button', { name: '본문 편집 저장', exact: true }).click();
  try {
    await until(
      () => main.evaluate(() => window.__manualSaveHeld),
      'actual manual-save request held',
    );
    assert.equal(
      await main.locator('.doc-user-edited').count(),
      1,
      'local marker is not a commit acknowledgment',
    );
    assert.equal(
      await blank.getByLabel('블록 Markdown', { exact: true }).inputValue(),
      '화면에서 채운 값',
    );
    const beforeCommit = (await invoke('doc_get', { id: uiDoc.id, revisionId: null })).revision;
    assert.notEqual(
      beforeCommit.blocks.find((block) => block.id === 'blank').markdown,
      '화면에서 채운 값',
    );
  } finally {
    await main.evaluate(() => window.__releaseManualSave?.());
  }
  let uiSaved;
  // The edited marker already exists in the local draft before Save. Wait for
  // the actual persisted revision, rather than using that marker as a commit ACK.
  await until(async () => {
    uiSaved = (await invoke('doc_get', { id: uiDoc.id, revisionId: null })).revision;
    return (
      uiSaved.operation === 'manual' &&
      uiSaved.blocks.find((block) => block.id === 'blank')?.markdown === '화면에서 채운 값'
    );
  }, 'manual edit is persisted before verification');
  await until(
    async () => (await main.locator('.doc-user-edited').count()) === 1,
    'only edited block marked',
  );
  assert.equal(uiSaved.blocks.find((block) => block.id === 'facts').userEdited, false);
  assert.equal(uiSaved.blocks.find((block) => block.id === 'blank').markdown, '화면에서 채운 값');
  await main.locator('.doc-edit-scroll').evaluate((element) => (element.scrollTop = 0));
  await main.screenshot({ path: join(evidence, 'documents-body-native.png') });
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await main.setViewportSize({ width: 760, height: 760 });
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await main.screenshot({ path: join(evidence, 'documents-body-760-native.png') });
  await main.setViewportSize({ width: 1280, height: 840 });
  await main.locator('.doc-history > summary').click();
  await main.getByRole('button', { name: '이전 버전으로 실행 취소', exact: true }).click();
  await until(
    async () =>
      (await invoke('doc_get', { id: uiDoc.id, revisionId: null })).revision.operation ===
      'restore',
    'UI undo creates revision',
  );
  assert.equal(
    (await invoke('doc_get', { id: uiDoc.id, revisionId: null })).revision.blocks.find(
      (block) => block.id === 'blank',
    ).markdown,
    '',
  );
  await main.getByRole('button', { name: '문서 삭제', exact: true }).click();
  await main.getByRole('button', { name: '모든 버전 삭제', exact: true }).click();
  await until(
    async () => (await invoke('doc_list', { query: '화면 검증', limit: 100 })).length === 0,
    'UI confirmed deletion',
  );
  await writeFile(source, 'shared changed source 경비 130000원');
  const sourceStates = await invoke('doc_current_citations', { id, revisionId: null });
  assert(sourceStates.some((citation) => citation.state === 'changed'));
  assert(
    (await invoke('doc_get', { id, revisionId: null })).revision.citations.every(
      (citation) => citation.state === 'current',
    ),
    'Current citation checks must not mutate revision snapshots',
  );
  const count = calls.length;
  const changed = await result('doc_generate_body', { request: request(restored) });
  assert.equal(changed.error.rag.code, 'sourceChanged');
  assert.equal(calls.length, count, 'Stale derived input must not be sent to the provider');
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  verifiedProfile = false;
  instance = await launch(profile, launchOptions);
  await verifyProfile(instance);
  assert.equal(
    (await instance.invoke('doc_get', { id, revisionId: null })).revision.id,
    restored.id,
  );
  assert.equal((await instance.invoke('rag_list', { query: '', limit: 100 })).length, 0);
  await instance.invoke('doc_delete', { id });
  assert.equal((await instance.invoke('doc_list', { query: '', limit: 100 })).length, 0);
  assert.deepEqual(instance.errors, []);
  assert.deepEqual(warnings, []);
  if (portable) assert.equal(await sha(executable), executableSha256);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      pdfDirectory,
      runtime,
      surface,
      executableSha256,
      warnings,
      errors: instance.errors,
      generation: 'synthetic loopback fixture',
      externalCalls: 0,
    }),
  );
} catch (error) {
  if (!verifiedProfile) throw new Error('Document fixture profile was not verified');
  console.error(JSON.stringify({ directory }));
  if (instance) {
    try {
      const editor = instance.main.locator('.doc-editor');
      if (await editor.count())
        console.error((await editor.innerText({ timeout: 1000 })).slice(0, 1600));
    } catch {
      // A diagnostic read must never replace the original assertion/IPC error.
    }
  }
  throw error;
} finally {
  if (instance) {
    if (verifiedProfile) await shutdown(instance);
    else {
      await instance.browser?.close().catch(() => {});
      instance.child?.kill();
    }
  }
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
