// Browser plugin not available: actual bundled Windows WebView2 via Playwright.
// Real pinned embeddings, SQLite/Lance and rendered handoff; generation alone
// uses a synthetic loopback provider. Every source and profile is fixture-owned.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply an explicit candidate executable');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile));
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const directory = await mkdtemp(join(tmpdir(), 'prism-chat-ranked-'));
const portable = join(directory, 'portable');
const data = join(portable, 'PrismData');
const folder = join(directory, 'source-documents');
await Promise.all([data, folder].map((path) => mkdir(path, { recursive: true })));
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
await writeFile(join(portable, 'prism.portable'), '');
writePackageNotices(portable, loadPackageNotices(original));
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
const executableSha256 = await hash(original);
assert.equal(await hash(executable), executableSha256);
const query = '제주 출장 경비';
for (let index = 0; index < 120; index++)
  await writeFile(
    join(folder, `rank-${String(index).padStart(3, '0')}.md`),
    `${query} 항공료 ${120000 + index}원 숙박비 80000원. 기록 번호 ${index}.`,
  );
// Several chunks from one file must still become exactly one selected file.
await writeFile(join(folder, 'multi-chunk.md'), `${query} 항공료 숙박비 영수증 정산. `.repeat(240));
const requests = [];
const sockets = new Set();
const server = createServer((request, response) => {
  const buffers = [];
  request.on('data', (chunk) => buffers.push(chunk));
  request.on('end', () => {
    requests.push(JSON.parse(Buffer.concat(buffers).toString()));
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({
        choices: [{ message: { content: '합성 근거 응답 [S1]' }, finish_reason: 'stop' }],
      }),
    );
  });
});
server.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    shortcut: 'Ctrl+Alt+Shift+F8',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    generation: {
      topK: 4,
      local: {
        endpoint: `http://127.0.0.1:${server.address().port}/v1`,
        model: 'synthetic-generation',
        maxTokens: 128,
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
let instance;
const results = [];
try {
  instance = await launch(data, { executable, portable: true });
  const { main, quick, invoke } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.development, profile === '0');
  assert.equal(runtime.installKind, 'portable');
  await until(
    async () => {
      const status = await invoke('content_status');
      return status.phase === 'ready' && status.indexed === 121;
    },
    'all fixture documents indexed',
    120000,
  );
  console.log(JSON.stringify({ phase: 'indexed', directory }));
  await invoke('model_import', {
    path: resolve(process.env.PRISM_TEST_MODEL ?? '.dev/models/embeddinggemma-2-Q8_0.gguf'),
    runtime: false,
  });
  await invoke('model_import', {
    path: resolve(
      process.env.PRISM_TEST_MODEL_RUNTIME ?? '.dev/runtime/llama-b11960-bin-win-cpu-x64.zip',
    ),
    runtime: true,
  });
  await invoke('model_start');
  await until(
    async () => {
      const status = await invoke('content_status');
      return status.phase === 'ready' && status.embedded === 121 && !status.embeddingError;
    },
    'actual pinned model embeds all fixture files',
    600000,
  );
  console.log(JSON.stringify({ phase: 'embedded', directory, executableSha256 }));
  const conversationPage = (id) =>
    invoke('rag_conversation', { id, beforeSequence: null, limit: 100 });
  for (const [name, mode, hybridSearch, spotlight] of [
    ['semantic', 'semantic', false, false],
    ['hybrid', 'semantic', true, false],
    ['automatic cosine', 'auto', false, true],
    ['automatic hybrid', 'auto', true, true],
  ]) {
    await main.getByRole('button', { name: '검색', exact: true }).click();
    const settings = await invoke('get_settings');
    await invoke('save_settings', {
      settings: { ...settings, hybridSearch, automaticSemantic: true },
      expectedRevision: settings.revision,
    });
    const session = await invoke('get_search_session');
    await invoke('set_search_session', {
      session: { ...session, mode, query, extension: '', scopeId: 'docs' },
    });
    await until(
      async () =>
        (await main
          .getByRole('button', { name: mode === 'auto' ? '자동' : '자연어', exact: true })
          .getAttribute('aria-pressed')) === 'true',
      `${name} session reaches the rendered search mode`,
    );
    const surface = spotlight ? quick : main;
    if (spotlight) await invoke('show_quick');
    const rows = surface.locator('.content-result-row');
    const chunkLimit = hybridSearch ? 100 : 20;
    await until(
      async () => (await rows.count()) === chunkLimit,
      `${name} displays top ${chunkLimit} chunks`,
      60000,
    );
    const shown = await surface
      .locator('.content-result-path')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('title')));
    const expected = [...new Set(shown)];
    assert(expected.length > 0 && expected.length < 121, 'some corpus files are outside top-N');
    assert.equal(shown.length, chunkLimit);
    const previous = new Set(
      (await invoke('rag_list', { query: '', limit: 100 })).map((c) => c.id),
    );
    const callsBefore = requests.length;
    await surface.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
    let chat;
    await until(async () => {
      chat = (await invoke('rag_list', { query: '', limit: 100 })).find((c) => !previous.has(c.id));
      return chat;
    }, `${name} creates its actual saved conversation`);
    const saved = await conversationPage(chat.id);
    assert.equal(saved.selectionSet, null, 'ranked handoff uses an explicit fixed membership');
    assert.deepEqual(
      saved.selection.documents,
      expected,
      `${name}: every displayed file in rank order`,
    );
    assert.deepEqual(saved.selection.selectedDocuments, expected);
    assert.equal(requests.length, callsBefore, 'handoff does not automatically ask a question');
    const panel = main.locator('.chat-search-selection');
    await panel.waitFor();
    await until(
      async () =>
        (await panel.locator('summary').innerText()).includes(
          `${expected.length}/${expected.length}`,
        ),
      `${name} main opens the exact selection`,
    );
    await panel.locator('summary').click();
    assert.equal(await panel.getByRole('checkbox').count(), expected.length);
    // Editing remains available and persists without widening membership.
    await panel.getByRole('checkbox').first().uncheck();
    await panel.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
    await until(
      async () =>
        (await conversationPage(chat.id)).selection.selectedDocuments.length ===
        expected.length - 1,
      `${name} edited selection persists`,
    );
    await main.getByRole('textbox', { name: '질문', exact: true }).fill(query);
    await main.getByRole('button', { name: '질문 전송', exact: true }).click();
    await until(
      () => requests.length === callsBefore + 1,
      `${name} sends only after explicit question`,
      120000,
    );
    await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').waitFor();
    await until(
      async () =>
        (await conversationPage(chat.id)).turns.some(
          (turn) => turn.role === 'assistant' && turn.status === 'complete',
        ) &&
        (await main.getByRole('button', { name: '응답 생성 중지', exact: true }).count()) === 0,
      `${name} answer is persisted and the composer has left its busy state`,
    );
    const answered = await conversationPage(chat.id);
    const user = answered.turns.find((turn) => turn.role === 'user');
    assert(user.provenance.length > 0);
    assert(
      user.provenance.every((source) => expected.slice(1).includes(source.filePath)),
      'RAG evidence stays inside the edited selection',
    );
    assert(
      !JSON.stringify(requests.at(-1)).includes(directory),
      'absolute paths stay outside generation input',
    );
    results.push({
      name,
      chunks: shown.length,
      files: expected.length,
      selected: expected.length - 1,
      spotlight,
    });
    console.log(JSON.stringify({ phase: 'handoff-verified', ...results.at(-1) }));
    await panel.locator('summary').click();
    await main.screenshot({
      path: join(directory, `${mode}-${hybridSearch ? 'hybrid' : 'cosine'}.png`),
      animations: 'disabled',
    });
  }
  assert.equal(requests.length, 4);
  assert(
    results.some((result) => result.chunks > result.files),
    'duplicate file chunks were exercised',
  );
  assert.equal(await main.title(), 'Prism');
  assert.equal(await main.locator('vite-error-overlay').count(), 0);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      executableSha256,
      release: profile === '1',
      model: 'actual pinned EmbeddingGemma 2 Q8',
      generation: 'synthetic loopback',
      results,
      externalAIRequests: 0,
      errors: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
