// Real filename/content/RAG/SQLite/UI flow, synthetic loopback generation only.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-search-chat-'));
const docs = join(directory, 'docs'),
  names = join(directory, 'names');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all([docs, names, evidence].map((path) => mkdir(path, { recursive: true })));
const approved = join(docs, '선택-공개.md'),
  privateFile = join(docs, '선택-개인.md');
await writeFile(approved, 'shared approvedcontext 공용 근거 '.repeat(90));
await writeFile(privateFile, 'shared privatecanary 개인 근거');
await Promise.all(
  Array.from({ length: 205 }, (_, i) => writeFile(join(names, `표본-${i}.md`), 'not indexed')),
);
const requests = [],
  sockets = new Set();
const server = createServer((request, response) => {
  const data = [];
  request.on('data', (chunk) => data.push(chunk));
  request.on('end', () => {
    requests.push(JSON.parse(Buffer.concat(data).toString()));
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({
        choices: [
          {
            message: {
              content: requests.length === 1 ? 'PRIVATE_REPLY_TOKEN [S1]' : '공용 근거 응답 [S1]',
            },
            finish_reason: 'stop',
          },
        ],
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
    shortcut: 'Ctrl+Alt+Shift+F24',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    generation: {
      local: {
        endpoint: `http://127.0.0.1:${server.address().port}/v1`,
        model: 'synthetic',
        maxTokens: 256,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
    scopes: [
      {
        id: 'docs',
        name: '문서',
        path: docs,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
      {
        id: 'names',
        name: '파일명 전용',
        path: names,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);
let instance;
try {
  instance = await launch(directory);
  let { main, invoke } = instance;
  assert.equal((await invoke('runtime_info')).shortcutError, null);
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'only permitted content indexed');
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('선택-');
  await until(async () => (await main.getByRole('option').count()) === 2, 'two filename matches');
  // The ordinary Chat tab also carries the current result set.
  await main.getByRole('button', { name: '채팅', exact: true }).click();
  const panel = main.locator('.chat-search-selection');
  await panel.getByText('검색 결과 파일 2/2개 선택', { exact: true }).waitFor();
  assert.equal(requests.length, 0, 'switching to chat must not send a question');
  await main.getByRole('textbox', { name: '질문', exact: true }).fill('shared');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () =>
      requests.length === 1 &&
      (await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').count()) ===
        1,
    'first answer',
  );
  assert(JSON.stringify(requests[0]).includes('privatecanary'));
  await panel.locator('summary').click();
  await panel.getByRole('checkbox', { name: /선택-개인.md/ }).uncheck();
  await main.getByRole('textbox', { name: '질문', exact: true }).fill('shared');
  assert(await main.getByRole('button', { name: '질문 전송', exact: true }).isDisabled());
  await panel.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
  await until(
    async () => await main.getByRole('button', { name: '질문 전송', exact: true }).isEnabled(),
    'saved selection enables sending',
  );
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () =>
      requests.length === 2 &&
      (await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').count()) ===
        2,
    'filtered answer',
  );
  assert(JSON.stringify(requests[1]).includes('approvedcontext'));
  assert(!JSON.stringify(requests[1]).includes('privatecanary'));
  assert(!JSON.stringify(requests[1]).includes('PRIVATE_REPLY_TOKEN'));
  assert.equal(requests[1].messages.length, 2, 'both roles of tainted history are omitted');
  assert(
    !JSON.stringify(requests).includes(directory),
    'target paths are not sent to the provider',
  );
  const conversation = (await invoke('rag_list', { query: '선택-', limit: 100 }))[0];
  assert.equal(
    (await invoke('rag_conversation', { id: conversation.id, beforeSequence: null, limit: 100 }))
      .selection.selectedDocuments.length,
    1,
  );
  await panel.locator('summary').scrollIntoViewIfNeeded();
  await main.locator('.chat-workspace').evaluate((element) => {
    element.scrollTop = 0;
  });
  await main.screenshot({ path: join(evidence, 'search-chat-main.png'), animations: 'disabled' });
  await panel.getByRole('button', { name: '파일 선택 해제', exact: true }).click();
  await panel.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
  await main.getByRole('textbox', { name: '질문', exact: true }).fill('shared');
  assert(await main.getByRole('button', { name: '질문 전송', exact: true }).isDisabled());
  // Even a direct caller cannot broaden an explicitly empty saved selection.
  const blocked = await main.evaluate(async (id) => {
    try {
      await window.__TAURI_INTERNALS__.invoke('rag_ask', {
        request: {
          requestId: crypto.randomUUID(),
          conversationId: id,
          question: 'shared',
          scopeIds: ['docs'],
          mode: 'documents',
          sourcesHidden: false,
          documentPaths: null,
        },
      });
      return 'unexpected';
    } catch (error) {
      return error.code;
    }
  }, conversation.id);
  assert.equal(blocked, 'noSources');
  assert.equal(requests.length, 2);
  // Body results can contain several chunks from one file; transfer file IDs once.
  await main.getByRole('button', { name: '검색', exact: true }).click();
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('shared');
  await until(
    async () => (await main.locator('.content-panel').getByRole('option').count()) > 2,
    'multiple content chunks across two files',
  );
  await main.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
  const bodySelection = main.locator('.chat-search-selection');
  await bodySelection.getByText('검색 결과 파일 2/2개 선택', { exact: true }).waitFor();
  await bodySelection.locator('summary').click();
  assert.equal(await bodySelection.getByRole('checkbox').count(), 2);
  await bodySelection
    .getByRole('button', { name: '파일 제한 해제하고 폴더 범위 사용', exact: true })
    .click();
  await until(
    async () => (await main.locator('.chat-search-selection').count()) === 0,
    'explicit removal returns to folder selection',
  );
  assert.equal(requests.length, 2, 'neither handoff nor scope editing sends a question');
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory);
  ({ main, invoke } = instance);
  assert.equal((await invoke('runtime_info')).shortcutError, null);
  const persisted = await invoke('rag_conversation', {
    id: conversation.id,
    beforeSequence: null,
    limit: 100,
  });
  assert.deepEqual(persisted.selection.selectedDocuments, []);
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('표본');
  await until(
    async () => (await main.getByRole('option').count()) === 200,
    'bounded filename result set',
  );
  await main.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
  const partial = main.locator('.chat-search-selection');
  await partial.locator('summary').click();
  await partial
    .getByText(
      '검색 상한에 따라 표시된 파일만 전달했습니다. 더 넓은 결과가 필요하면 검색 범위를 조정하세요.',
      { exact: true },
    )
    .waitFor();
  assert.equal(await partial.getByRole('checkbox').count(), 200);
  await main.setViewportSize({ width: 390, height: 844 });
  await partial.locator('summary').scrollIntoViewIfNeeded();
  await partial.evaluate((element) => element.scrollIntoView({ block: 'start' }));
  await main.screenshot({ path: join(evidence, 'search-chat-mobile.png'), animations: 'disabled' });
  assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      generation: 'synthetic loopback',
      externalCalls: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
