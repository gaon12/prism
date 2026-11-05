// Actual pinned EmbeddingGemma 2 + FTS/Lance + native RAG. Generation alone is
// synthetic loopback; every document and profile belongs to this fixture.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-rag-hybrid-'));
// "documents" under the app profile is reserved for private generated drafts.
// Source fixtures must not collide with that intentionally excluded cache.
const folder = join(directory, 'source-documents');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all([folder, evidence].map((path) => mkdir(path, { recursive: true })));
for (let index = 0; index < 266; index++) {
  await writeFile(
    join(folder, `rank-${String(index).padStart(3, '0')}.md`),
    index < 256
      ? `정원 관리와 고양이 사료 안내. 화분 물주기 기록 ${index}.`
      : `제주 출장 경비 항공료 ${120000 + index}원 숙박비 80000원. ${'경비 '.repeat(index - 255)}`,
  );
}
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
        choices: [{ message: { content: '합성 답변 [S1]' }, finish_reason: 'stop' }],
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
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F6',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    generation: {
      topK: 4,
      maxContextChars: 30000,
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
        name: 'synthetic',
        path: folder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
let instance;
try {
  instance = await launch(directory);
  const { main, invoke } = instance;
  await until(
    async () => {
      const s = await invoke('content_status');
      return s.phase === 'ready' && s.indexed === 266;
    },
    'all synthetic documents indexed',
    120000,
  );
  console.log(JSON.stringify({ directory, phase: 'sources-indexed' }));
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
    'real pinned model starts',
    120000,
  );
  await until(
    async () => {
      const s = await invoke('content_status');
      return s.phase === 'ready' && s.embedded === 266 && !s.embeddingError;
    },
    'real embeddings for both file pages',
    600000,
  );
  console.log(JSON.stringify({ directory, phase: 'actual-model-embedded' }));
  await until(async () => !(await invoke('index_status')).scanning, 'file catalog ready');
  const createChat = async () => {
    const files = await invoke('search_files', { query: 'rank-', extension: '', scopeId: 'docs' });
    const current = await invoke('get_settings');
    return invoke('rag_create_filename_search', {
      requestId: crypto.randomUUID(),
      search: {
        query: 'rank-',
        extension: '',
        scopeId: 'docs',
        indexRevision: files.status.revision,
        settingsRevision: current.revision,
      },
    });
  };
  const chat = await createChat();
  const page = await invoke('rag_file_set_page', { id: chat.id, offset: 0 });
  assert.equal(page.total, 266);
  const query = '경비';
  const search = (command) =>
    invoke(command, { query, extension: '', scopeId: 'docs', requestId: crypto.randomUUID() });
  const keyword = await search('search_content');
  const semantic = await search('search_semantic');
  const hybrid = await search('search_hybrid');
  const expected = {
    keyword: keyword.hits.slice(0, 4).map((hit) => hit.chunk.id),
    semantic: semantic.hits.slice(0, 4).map((hit) => hit.chunk.id),
    hybrid: hybrid.hits.slice(0, 4).map((hit) => hit.chunk.id),
  };
  assert(
    hybrid.hits
      .slice(0, 4)
      .every((hit) => /rank-26[0-5]\.md$|rank-25[6-9]\.md$/u.test(hit.chunk.filePath)),
    'top evidence lies beyond the first 256 filename members',
  );
  // Exercise actual rendered settings and persistence for all three modes.
  for (const mode of ['hybrid', 'semantic', 'keyword']) {
    await main.getByRole('button', { name: '설정', exact: true }).click();
    const dialog = main.getByRole('dialog');
    await dialog.getByRole('button', { name: 'AI·모델', exact: true }).click();
    const modeInput = dialog.getByLabel('채팅·문서의 근거 검색', { exact: true });
    await modeInput.scrollIntoViewIfNeeded();
    await modeInput.selectOption(mode);
    if (mode === 'hybrid')
      await main.screenshot({ path: join(evidence, 'rag-hybrid-settings.png') });
    await dialog.getByRole('button', { name: '저장', exact: true }).click();
    await until(
      async () => (await invoke('get_settings')).generation.retrieval === mode,
      `persist ${mode}`,
    );
    const conversation = mode === 'hybrid' ? chat : await createChat();
    const answer = await invoke('rag_ask', {
      request: {
        requestId: crypto.randomUUID(),
        conversationId: conversation.id,
        question: query,
        scopeIds: ['docs'],
        mode: 'documents',
        sourcesHidden: false,
      },
    });
    assert.deepEqual(
      answer.userTurn.provenance.map((source) => source.chunkId),
      expected[mode],
      `global ${mode} ranks across saved-file batches`,
    );
    assert(
      !JSON.stringify(requests.at(-1)).includes(directory),
      'absolute local paths stay outside provider input',
    );
    console.log(JSON.stringify({ directory, phase: 'retrieval-verified', mode }));
  }
  assert.equal(requests.length, 3);
  await main.setViewportSize({ width: 390, height: 760 });
  // The narrow layout hides the library rail; the status bar remains an
  // accessible settings entry point at this width.
  await main.locator('.statusbar').getByRole('button', { name: '한국어', exact: true }).click();
  await main.getByRole('button', { name: 'AI·모델', exact: true }).click();
  const retrieval = main.getByLabel('채팅·문서의 근거 검색', { exact: true });
  await retrieval.scrollIntoViewIfNeeded();
  assert.equal(await retrieval.inputValue(), 'keyword');
  await main.screenshot({ path: join(evidence, 'rag-hybrid-settings-mobile.png') });
  await main.getByRole('button', { name: '취소', exact: true }).click();
  assert.equal(
    JSON.parse(await readFile(join(directory, 'settings.json'), 'utf8')).generation.retrieval,
    'keyword',
  );
  assert.equal(await main.title(), 'Prism');
  assert.equal(await main.locator('vite-error-overlay').count(), 0);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      directory,
      documents: 266,
      model: 'actual pinned EmbeddingGemma 2 Q8',
      generation: 'synthetic loopback',
      modes: Object.keys(expected),
      externalAIRequests: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
