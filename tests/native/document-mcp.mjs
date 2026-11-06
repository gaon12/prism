// Actual document/RAG/SQLite/stdio MCP/UI flow. Only generation is synthetic
// loopback. Browser plugin not available: repository Playwright/WebView2.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply an explicit candidate executable');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile));
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const root = await mkdtemp(join(tmpdir(), 'prism-document-mcp-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
const folder = join(root, 'sources');
await Promise.all([data, folder].map((path) => mkdir(path, { recursive: true })));
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
await writeFile(join(portable, 'prism.portable'), '');
writePackageNotices(portable, loadPackageNotices(original));
const hash = async (path) => {
  const digest = createHash('sha256');
  for await (const bytes of createReadStream(path)) digest.update(bytes);
  return digest.digest('hex');
};
const executableSha256 = await hash(original);
assert.equal(await hash(executable), executableSha256);
const localCanary = 'LOCAL_SOURCE_PRIVATE_30261';
const templateCanary = 'PRIVATE_DRAFT_TEMPLATE_40261';
const instructionCanary = 'PRIVATE_GENERATION_INSTRUCTION_50261';
const externalCanary = 'EXTERNAL_SEARCH_EXCERPT_60261';
const source = join(folder, '원본.md');
await writeFile(source, `researchneedle ${localCanary} 승인된 로컬 근거 120000원.`);
const sourceSha256 = await hash(source);
const trace = join(root, 'mcp.jsonl');
const control = join(root, 'mcp-control.json');
await writeFile(trace, '');
await writeFile(control, JSON.stringify({ mode: 'normal', text: externalCanary }));
const descriptor = {
  id: 'doc-research',
  name: '문서용 격리 자료',
  command: process.execPath,
  args: [fileURLToPath(new URL('./fixtures/mcp-server.mjs', import.meta.url)), trace, control],
  allowedTools: [],
  search: { tool: 'search', argument: 'query' },
};
const requests = [];
const sockets = new Set();
const provider = createServer((request, response) => {
  const buffers = [];
  request.on('data', (bytes) => buffers.push(bytes));
  request.on('end', () => {
    const packet = JSON.parse(Buffer.concat(buffers).toString());
    const text = packet.messages.at(-1).content;
    const marker = 'Document input:\n';
    const input = JSON.parse(text.slice(text.lastIndexOf(marker) + marker.length).split('\n')[0]);
    requests.push({ packet, input });
    const payload =
      input.operation === 'outline'
        ? {
            title: 'researchneedle',
            sections: [{ id: 'summary', heading: '조사 결과', instruction: 'researchneedle' }],
          }
        : input.operation === 'rewrite'
          ? {
              blocks: [
                {
                  id: input.targetBlockIds[0],
                  sectionId: 'summary',
                  kind: 'fact',
                  markdown: 'REWRITTEN_DOCUMENT_CANARY [E1]',
                  sourceLabels: ['[E1]'],
                },
              ],
            }
          : {
              blocks: [
                {
                  id: 'facts',
                  sectionId: 'summary',
                  kind: 'fact',
                  markdown: 'GENERATED_DOCUMENT_CANARY [E1] [S1]',
                  sourceLabels: ['[E1]', '[S1]'],
                },
                {
                  id: 'retained',
                  sectionId: 'summary',
                  kind: 'aiProposal',
                  markdown: 'UNTOUCHED_BLOCK [E1]',
                  sourceLabels: ['[E1]'],
                },
              ],
            };
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({
        choices: [{ message: { content: JSON.stringify(payload) }, finish_reason: 'stop' }],
      }),
    );
  });
});
provider.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => provider.listen(0, '127.0.0.1', done));
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F9',
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    mcp: { enabled: true, servers: [descriptor] },
    generation: {
      retrieval: 'keyword',
      maxContextChars: 40000,
      local: {
        endpoint: `http://127.0.0.1:${provider.address().port}/v1`,
        model: 'synthetic-documents',
        maxTokens: 1024,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
    scopes: [
      {
        id: 'docs',
        name: '격리 자료',
        path: folder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
const events = async () =>
  (await readFile(trace, 'utf8')).split(/\r?\n/u).filter(Boolean).map(JSON.parse);
const calls = async () =>
  (await events()).filter((event) => event.message?.method === 'tools/call');
const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
let instance;
try {
  instance = await launch(data, { executable, portable: true });
  const { main, quick, invoke } = instance;
  assert.equal((await invoke('runtime_info')).development, profile === '0');
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 1;
  }, 'local fixture indexed');
  const save = async (change) => {
    const settings = await invoke('get_settings');
    return invoke('save_settings', {
      settings: change(settings),
      expectedRevision: settings.revision,
    });
  };
  const checked = (command, args) =>
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
  const created = await invoke('doc_create', {
    input: {
      title: '격리 MCP 문서',
      purpose: 'researchneedle',
      templateMarkdown: templateCanary,
      scopeIds: ['docs'],
    },
  });
  const current = () => invoke('doc_get', { id: created.document.id, revisionId: null });
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main
    .locator('.doc-sidebar')
    .getByRole('button')
    .filter({ hasText: '격리 MCP 문서' })
    .click();
  const outline = main.getByRole('button', { name: 'AI로 개요 생성', exact: true });
  await until(() => outline.isEnabled(), 'outline action ready');
  await outline.click();
  await until(
    async () => (await current()).revision.operation === 'outline',
    'outline without research',
  );
  assert.equal((await events()).length, 0, 'no opt-in means no server process or tool request');
  const openOptions = async () => {
    const options = main.locator('.doc-options');
    if ((await options.getAttribute('open')) === null) await options.locator('summary').click();
  };
  await openOptions();
  await main.getByLabel('추가 지시', { exact: true }).fill(instructionCanary);
  const optIn = main.getByLabel('이번 생성에서 외부 자료도 찾기', { exact: true });
  const query = main.getByLabel('MCP 검색 문장', { exact: true });
  await optIn.check();
  assert(await outline.isDisabled(), 'explicit opt-in also requires a query');
  await query.fill('ONLY_OUTLINE_RESEARCH_QUERY');
  await query.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(root, 'document-mcp-query.png'), animations: 'disabled' });
  await outline.click();
  await until(
    async () => (await current()).revision.provenance.some((ref) => ref.scopeId.startsWith('mcp_')),
    'actual MCP-backed outline',
  );
  await optIn.waitFor();
  assert.equal(await optIn.isChecked(), false, 'opt-in is cleared after each generation');
  assert.deepEqual(
    (await calls()).map((event) => event.message.params.arguments),
    [{ query: 'ONLY_OUTLINE_RESEARCH_QUERY' }],
  );
  let page = await current();
  await main.getByLabel('개요를 검토했습니다.', { exact: true }).check();
  await main.getByRole('button', { name: '검토한 개요 저장', exact: true }).click();
  const body = main.getByRole('button', { name: '검토한 개요로 본문 생성', exact: true });
  await until(() => body.isEnabled(), 'reviewed body generation action');
  await openOptions();
  await optIn.check();
  await query.fill('ONLY_BODY_RESEARCH_QUERY');
  await body.click();
  await until(
    async () => (await current()).revision.operation === 'body',
    'actual external body generation',
  );
  await main.locator('.doc-block').first().getByLabel('블록 Markdown', { exact: true }).waitFor();
  page = await current();
  assert(page.revision.blocks[0].markdown.includes('[E2]'));
  assert(page.revision.blocks[1].markdown.includes('[E2]'));
  assert.equal(await optIn.isChecked(), false);
  const options = main.locator('.doc-options');
  if ((await options.getAttribute('open')) !== null) await options.locator('summary').click();
  const sourceRecords = main.locator('.doc-source-records');
  await sourceRecords.locator(':scope > summary').click();
  await main.getByRole('heading', { name: '이 문서의 외부 자료', exact: true }).waitFor();
  await main.getByText(externalCanary, { exact: true }).first().waitFor();
  await main
    .getByRole('heading', { name: '이 문서의 외부 자료', exact: true })
    .scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(root, 'document-mcp-sources.png'), animations: 'disabled' });
  const rewrite = await invoke('doc_rewrite', {
    request: {
      documentId: created.document.id,
      expectedRevisionId: page.revision.id,
      requestId: crypto.randomUUID(),
      instruction: instructionCanary,
      scopeIds: ['docs'],
      researchQuery: 'ONLY_REWRITE_RESEARCH_QUERY',
      targetBlockIds: ['facts'],
      selection: null,
    },
  });
  assert(rewrite.blocks[0].markdown.includes('[E3]'));
  assert(rewrite.blocks[1].markdown.includes('[E2]'), 'untouched external citation stays bound');
  await until(
    async () =>
      (
        await main
          .locator('.doc-block')
          .first()
          .getByLabel('블록 Markdown', { exact: true })
          .inputValue()
      ).includes('REWRITTEN_DOCUMENT_CANARY'),
    'UI receives actual rewrite',
  );
  const exported = await invoke('doc_export', {
    id: created.document.id,
    revision: rewrite.id,
    includeSources: true,
    includeTypeMarkers: false,
  });
  assert(exported.includes('[^E3]') && exported.includes('mcp://doc-research/search'));
  assert.equal(
    (await invoke('doc_external_sources', { id: created.document.id, revisionId: rewrite.id }))
      .length,
    3,
  );
  const transmitted = (await calls()).map((event) => event.message.params.arguments);
  assert.deepEqual(transmitted, [
    { query: 'ONLY_OUTLINE_RESEARCH_QUERY' },
    { query: 'ONLY_BODY_RESEARCH_QUERY' },
    { query: 'ONLY_REWRITE_RESEARCH_QUERY' },
  ]);
  for (const canary of [localCanary, templateCanary, instructionCanary, folder])
    assert(!JSON.stringify(transmitted).includes(canary));
  assert(
    JSON.stringify(requests).includes(templateCanary),
    'generation still receives the permitted draft context',
  );
  assert.equal(requests.length, 4);
  await writeFile(control, JSON.stringify({ mode: 'stall', text: externalCanary }));
  const cancelId = crypto.randomUUID();
  const pending = checked('doc_rewrite', {
    request: {
      documentId: created.document.id,
      expectedRevisionId: rewrite.id,
      requestId: cancelId,
      instruction: '',
      scopeIds: ['docs'],
      researchQuery: 'ONLY_CANCEL_RESEARCH_QUERY',
      targetBlockIds: ['facts'],
      selection: null,
    },
  });
  await until(
    async () => (await calls()).length === 4,
    'actual MCP tool call waits for cancellation',
  );
  await invoke('doc_cancel', { id: created.document.id, request: cancelId });
  const cancelled = await pending;
  assert.equal(cancelled.ok, false);
  assert.equal(cancelled.error.code, 'cancelled');
  assert.equal(requests.length, 4, 'cancelled research does not reach generation');
  assert.equal((await current()).revision.id, rewrite.id);
  await until(
    async () =>
      (await events())
        .filter((event) => event.event === 'start')
        .every((event) => !alive(event.pid)),
    'all owned MCP processes stop',
  );
  await until(
    async () =>
      !['outline', 'body', 'rewrite'].includes(
        (await invoke('doc_job', { id: created.document.id }))?.phase,
      ),
    'document job is no longer active',
  );
  await writeFile(control, JSON.stringify({ mode: 'normal', text: '' }));
  const emptyDoc = await invoke('doc_create', {
    input: {
      title: '결과 없음 상태 검증',
      purpose: 'researchneedle',
      templateMarkdown: null,
      scopeIds: ['docs'],
    },
  });
  const emptyOutline = await invoke('doc_generate_outline', {
    request: {
      documentId: emptyDoc.document.id,
      expectedRevisionId: emptyDoc.revision.id,
      requestId: crypto.randomUUID(),
      instruction: '',
      scopeIds: ['docs'],
      researchQuery: 'EMPTY_RESEARCH_QUERY',
    },
  });
  const emptyResults = await invoke('doc_external_sources', {
    id: emptyDoc.document.id,
    revisionId: emptyOutline.id,
  });
  assert.equal(emptyResults.length, 1);
  assert.equal(emptyResults[0].source, null);
  assert.equal(emptyResults[0].evidence.outcome, 'empty');
  assert.equal(requests.length, 5);
  const edit = main.locator('.doc-block').first().getByLabel('블록 Markdown', { exact: true });
  const draft = 'UNSAVED_EDIT_RETAINED';
  await edit.fill(draft);
  await save((settings) => ({ ...settings, mcp: { ...settings.mcp, servers: [] } }));
  await main.getByRole('alert').filter({ hasText: '원래 MCP 서버 설정' }).waitFor();
  assert.equal(await main.locator('.doc-block').count(), 0, 'revoked draft text is not rendered');
  assert(!(await main.locator('body').innerText()).includes(draft));
  for (const command of ['doc_get', 'doc_export', 'doc_external_sources']) {
    const args =
      command === 'doc_export'
        ? {
            id: created.document.id,
            revision: rewrite.id,
            includeSources: true,
            includeTypeMarkers: false,
          }
        : { id: created.document.id, revisionId: rewrite.id };
    const result = await checked(command, args);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'externalRevoked');
  }
  for (const [command, extension] of [
    ['doc_export_pdf', 'pdf'],
    ['doc_export_hwpx', 'hwpx'],
  ]) {
    const path = join(root, `retained.${extension}`);
    await writeFile(path, 'PREVIOUS_EXPORT');
    const result = await checked(command, {
      id: created.document.id,
      revision: rewrite.id,
      includeSources: true,
      includeTypeMarkers: false,
      request: crypto.randomUUID(),
      path,
    });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'externalRevoked');
    assert.equal(await readFile(path, 'utf8'), 'PREVIOUS_EXPORT');
  }
  await main.screenshot({ path: join(root, 'document-mcp-revoked.png'), animations: 'disabled' });
  await save((settings) => ({ ...settings, mcp: { enabled: true, servers: [descriptor] } }));
  await edit.waitFor();
  assert.equal(await edit.inputValue(), draft, 'permission recovery retains unsaved edits');
  assert.equal(
    (await current()).revision.id,
    rewrite.id,
    'permission recovery does not overwrite immutable storage',
  );
  assert.equal(
    (await calls()).length,
    5,
    'viewing/restoring sources never starts another MCP search',
  );
  const starts = (await events()).filter((event) => event.event === 'start');
  assert.equal(starts.length, 5, 'one owned stdio process for each explicit research request');
  await until(() => starts.every((event) => !alive(event.pid)), 'all owned MCP processes reaped');
  await main.setViewportSize({ width: 390, height: 760 });
  assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await main.screenshot({ path: join(root, 'document-mcp-narrow.png'), animations: 'disabled' });
  const quickDenied = await quick.evaluate(
    async ({ id, revisionId }) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_validate_access', { id, revisionId });
        return null;
      } catch (error) {
        return error.code;
      }
    },
    { id: created.document.id, revisionId: rewrite.id },
  );
  assert.equal(quickDenied, 'input');
  assert.equal(await hash(source), sourceSha256);
  assert.equal((await invoke('model_status')).modelReady, false);
  assert.equal(await main.title(), 'Prism');
  assert.equal(await main.locator('vite-error-overlay').count(), 0);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      executableSha256,
      release: profile === '1',
      queries: (await calls()).map((event) => event.message.params.arguments.query),
      generationRequests: requests.length,
      unsavedEditsRetained: true,
      revokedExportsPreserveFiles: true,
      originalUnchanged: true,
      externalAIRequests: 0,
      errors: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => provider.close(done));
}
