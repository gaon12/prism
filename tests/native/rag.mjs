// Actual native RAG/SQLite/DPAPI/source resolver, with only a synthetic loopback
// generation provider. No real API key, external provider or user file is used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-rag-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
const publicFolder = join(directory, 'public-documents'),
  privateFolder = join(directory, 'private-documents');
await Promise.all([publicFolder, privateFolder].map((path) => mkdir(path)));
await writeFile(join(publicFolder, '경비.md'), 'shared public 제주도 경비 120000원');
await writeFile(join(privateFolder, '개인.md'), 'shared privatecanary local-only evidence');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'public',
        name: 'public',
        path: publicFolder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
      {
        id: 'private',
        name: 'private',
        path: privateFolder,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
const requests = [],
  sockets = new Set();
let behavior = 'normal';
const server = createServer((request, response) => {
  const buffers = [];
  request.on('data', (chunk) => buffers.push(chunk));
  request.on('end', () => {
    requests.push(JSON.parse(Buffer.concat(buffers).toString()));
    if (behavior === 'stall') return;
    const body = JSON.stringify(
      behavior === 'error'
        ? { error: { message: 'synthetic unavailable' } }
        : {
            model: 'synthetic-generation',
            choices: [
              { message: { content: '경비는 120000원입니다. [S1] [S999]' }, finish_reason: 'stop' },
            ],
            usage: { prompt_tokens: 12, completion_tokens: 5, total_tokens: 17 },
          },
    );
    response.writeHead(behavior === 'error' ? 503 : 200, {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(body),
    });
    response.end(body);
  });
});
server.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const endpoint = `http://127.0.0.1:${server.address().port}/v1`;
let instance;
try {
  instance = await launch(directory);
  let { invoke, main } = instance;
  const result = (command, args) =>
    main.evaluate(
      async ({ command, args }) => {
        try {
          return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(command, args) };
        } catch (error) {
          return { ok: false, error: { code: error?.code, message: error?.message } };
        }
      },
      { command, args },
    );
  const save = async (transform) => {
    const current = await invoke('get_settings');
    return invoke('save_settings', {
      settings: transform(current),
      expectedRevision: current.revision,
    });
  };
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'native document indexing');
  await save((settings) => ({
    ...settings,
    generation: {
      ...settings.generation,
      local: {
        endpoint,
        model: 'synthetic-generation',
        maxTokens: 256,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
  }));
  const conversation = await invoke('rag_create', { title: '합성 근거 대화' });
  const ask = (question, scopeIds = ['public', 'private'], mode = 'documents') => ({
    requestId: crypto.randomUUID(),
    conversationId: conversation.id,
    question,
    scopeIds,
    mode,
    sourcesHidden: false,
  });
  const answer = await invoke('rag_ask', { request: ask('shared') });
  assert.equal(answer.assistantTurn.claimsVerified, false);
  assert.equal(
    answer.assistantTurn.evidence,
    'unverified',
    'unknown labels must not imply verified citation coverage',
  );
  assert(
    answer.assistantTurn.citations.some((citation) => citation.label === 'S1' && citation.isCited),
  );
  assert(answer.assistantTurn.unknownCitations.includes('S999'));
  assert.equal(
    answer.assistantTurn.usage.total_tokens,
    17,
    'provider usage must be preserved, not estimated',
  );
  assert(requests[0].messages.some((message) => message.content.includes('privatecanary')));
  assert(
    !JSON.stringify(requests[0]).includes(directory),
    'absolute local paths must stay out of provider input',
  );
  await invoke('rag_sources_hidden', { turnId: answer.assistantTurn.id, hidden: true });
  await save((settings) => ({
    ...settings,
    scopes: settings.scopes.map((scope) =>
      scope.id === 'private' ? { ...scope, contentEnabled: false } : scope,
    ),
  }));
  await until(
    async () => (await invoke('content_status')).phase === 'ready',
    'scope removal reconciliation',
  );
  await invoke('rag_ask', { request: ask('shared', ['public']) });
  assert.equal(
    requests.at(-1).messages.length,
    2,
    'both roles of history tainted by private evidence must be omitted',
  );
  assert(!JSON.stringify(requests.at(-1)).includes('privatecanary'));
  const count = requests.length;
  const empty = await result('rag_ask', { request: ask('unmatchablexyz', ['public']) });
  assert.equal(empty.ok, false);
  assert.equal(empty.error.code, 'noSources');
  assert.equal(requests.length, count, 'no evidence must not silently trigger general generation');
  const slot = 'external',
    endpointA = 'https://example.org/v1',
    endpointB = 'https://example.net/v1';
  const fakeKey = 'synthetic-endpoint-bound-key';
  await invoke('generation_set_key', { slot, endpoint: endpointA, key: fakeKey });
  assert.equal(
    (await invoke('generation_credential_status', { slot, endpoint: endpointA })).hasKey,
    true,
  );
  assert.equal(
    (await invoke('generation_credential_status', { slot, endpoint: endpointB })).hasKey,
    false,
  );
  assert(!(await readFile(join(directory, 'settings.json'), 'utf8')).includes(fakeKey));
  await save((settings) => ({
    ...settings,
    generation: {
      ...settings.generation,
      mode: 'external',
      allowExternalQuestions: false,
      external: {
        endpoint: endpointA,
        model: 'synthetic-remote',
        maxTokens: 256,
        tokenParameter: 'maxCompletionTokens',
        temperature: null,
      },
    },
  }));
  const blocked = await result('rag_ask', { request: ask('shared', ['public']) });
  assert.equal(blocked.error.code, 'scopeBlocked');
  await save((settings) => ({
    ...settings,
    generation: { ...settings.generation, allowExternalQuestions: true },
  }));
  const blockedContent = await result('rag_ask', { request: ask('shared', ['public']) });
  assert.equal(
    blockedContent.error.code,
    'scopeBlocked',
    'global question opt-in must not authorize folder content',
  );
  assert.equal(requests.length, count);
  await save((settings) => ({
    ...settings,
    generation: { ...settings.generation, mode: 'local' },
  }));
  const general = await invoke('rag_ask', {
    request: ask('general synthetic question', [], 'general'),
  });
  assert.equal(general.assistantTurn.evidence, 'unverified');
  assert.equal(general.assistantTurn.citations.length, 0);
  behavior = 'stall';
  const pendingRequest = ask('shared', ['public']);
  const before = requests.length;
  const pending = result('rag_ask', { request: pendingRequest });
  await until(() => requests.length > before, 'synthetic stalled provider receives request');
  await invoke('rag_cancel', {
    conversationId: conversation.id,
    requestId: pendingRequest.requestId,
  });
  assert.equal((await pending).error.code, 'cancelled');
  behavior = 'normal';
  const page = await invoke('rag_conversation', {
    id: conversation.id,
    beforeSequence: null,
    limit: 100,
  });
  assert(page.turns.some((turn) => turn.status === 'cancelled'));
  assert(page.turns.some((turn) => turn.id === answer.assistantTurn.id && turn.sourcesHidden));
  const markdown = await invoke('rag_export', { id: conversation.id, includeSources: false });
  assert(markdown.includes('120000'));
  assert(!markdown.includes('Sources (references'));
  await invoke('rag_rename', { id: conversation.id, title: '이름을 바꾼 대화' });
  assert.equal(
    (await invoke('rag_list', { query: '이름을 바꾼', limit: 100 }))[0].id,
    conversation.id,
  );
  await invoke('generation_clear_key', { slot, endpoint: endpointA });
  assert.equal(
    (await invoke('generation_credential_status', { slot, endpoint: endpointA })).hasKey,
    false,
  );
  // Exercise the real chat controls after backend policy checks, in a separate
  // conversation so UI history cannot change the earlier taint assertions.
  await main.getByRole('button', { name: '채팅', exact: true }).click();
  await main.getByRole('heading', { name: 'RAG 대화', exact: true }).waitFor();
  await main.getByRole('button', { name: '새 대화', exact: true }).click();
  const question = main.getByRole('textbox', { name: '질문', exact: true });
  await question.fill('shared');
  await main.locator('.chat-retrieval-options > summary').click();
  const selectedScopes = main.getByRole('group', { name: '문서 참조 폴더 선택', exact: true });
  await selectedScopes.getByRole('checkbox').uncheck();
  assert.equal(
    await main.getByRole('button', { name: '질문 전송', exact: true }).isDisabled(),
    true,
    'Unchecked document scopes must not become backend all-scopes',
  );
  await selectedScopes.getByRole('checkbox').check();
  await main.locator('.chat-retrieval-options > summary').click();
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  const completed = main.locator('.chat-turn[data-role="assistant"][data-status="complete"]');
  await completed.waitFor();
  const evidenceText = await completed.locator('.chat-evidence').innerText();
  assert(
    evidenceText.includes('출처를 다시 확인'),
    `Unexpected evidence label: ${evidenceText}; ${await completed.innerText()}`,
  );
  assert((await completed.innerText()).includes('S999'));
  await completed.getByRole('button', { name: '현재 본문 확인', exact: true }).click();
  await main.locator('.chat-preview pre').waitFor();
  assert((await main.locator('.chat-preview').innerText()).includes('120000'));
  await main.screenshot({ path: join(evidence, 'rag-chat-main.png') });
  await writeFile(join(publicFolder, '경비.md'), 'shared changed public 제주도 경비 130000원');
  await until(
    async () => (await main.locator('.chat-preview pre').count()) === 0,
    'changed source preview must be removed',
  );
  const closePreview = main.getByRole('button', { name: '미리보기 닫기', exact: true });
  if (await closePreview.count()) await closePreview.click();
  await completed.getByRole('button', { name: '이 답변의 출처 숨기기', exact: true }).click();
  await completed.getByRole('button', { name: '이 답변의 출처 표시', exact: true }).waitFor();
  await main.getByRole('button', { name: '이름 변경', exact: true }).click();
  await main.getByRole('textbox', { name: '대화 이름', exact: true }).fill('UI 검증 대화');
  await main.getByRole('button', { name: '이름 저장', exact: true }).click();
  await until(
    async () =>
      (await main.locator('.chat-conversation-list').innerText()).includes('UI 검증 대화'),
    'renamed conversation list',
  );
  await until(
    async () => (await invoke('content_status')).phase === 'ready',
    'changed source reindexed',
  );
  behavior = 'stall';
  const uiBefore = requests.length;
  await question.fill('shared');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(() => requests.length > uiBefore, 'UI request reaches synthetic provider');
  await question.fill('새로 작성 중인 질문');
  await main.getByRole('button', { name: '응답 생성 중지', exact: true }).click();
  await main.locator('.chat-turn[data-role="assistant"][data-status="cancelled"]').waitFor();
  assert.equal(
    await question.inputValue(),
    '새로 작성 중인 질문',
    'A completed old request must not erase newly typed text',
  );
  behavior = 'normal';
  await main.screenshot({ path: join(evidence, 'rag-chat-cancelled.png') });
  await main.getByRole('button', { name: '대화 삭제', exact: true }).click();
  await main.getByRole('button', { name: '영구 삭제', exact: true }).click();
  await until(
    async () => (await invoke('rag_list', { query: 'UI 검증 대화', limit: 100 })).length === 0,
    'UI deletion persisted',
  );
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory);
  const persisted = await instance.invoke('rag_conversation', {
    id: conversation.id,
    beforeSequence: null,
    limit: 100,
  });
  assert.equal(persisted.conversation.title, '이름을 바꾼 대화');
  assert(persisted.turns.some((turn) => turn.status === 'cancelled'));
  await instance.invoke('rag_delete', { id: conversation.id });
  assert.equal((await instance.invoke('rag_list', { query: '', limit: 100 })).length, 0);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      generation: 'synthetic loopback fixture',
      externalCalls: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
