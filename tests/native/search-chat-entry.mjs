// Real content enumeration, Spotlight/main IPC and persisted chat selection.
// Only synthetic documents and a loopback generation provider are used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-chat-entry-'));
const docs = join(directory, 'docs'),
  metadata = join(directory, 'metadata');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all([docs, metadata, evidence].map((path) => mkdir(path, { recursive: true })));
for (let index = 0; index < 107; index++)
  await writeFile(
    join(docs, `entry-${String(index).padStart(3, '0')}.md`),
    `entryneedle commoncontext ${index === 106 ? 'unseenlastneedle TAIL_EVIDENCE_107' : ''}`,
  );
await writeFile(join(metadata, 'entry-denied.md'), 'entryneedle hiddenmetadata');
const calls = [],
  sockets = new Set();
const server = createServer((request, response) => {
  const body = [];
  request.on('data', (chunk) => body.push(chunk));
  request.on('end', () => {
    calls.push(JSON.parse(Buffer.concat(body).toString()));
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
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F23',
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
        id: 'metadata',
        name: '이름 전용',
        path: metadata,
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
  const { main, quick, invoke } = instance;
  await until(
    async () => {
      const status = await invoke('content_status');
      return status.phase === 'ready' && status.indexed === 107;
    },
    'all synthetic content indexed',
    60_000,
  );
  await until(async () => {
    const status = await invoke('index_status');
    return !status.scanning && status.count >= 108;
  }, 'filename catalog ready');
  assert.equal((await invoke('model_status')).modelReady, false);
  assert.equal((await invoke('runtime_info')).shortcutError, null);
  assert.equal(
    await quick.getByRole('button', { name: '검색 결과로 대화', exact: true }).count(),
    0,
  );
  assert.equal(await quick.locator('.quick-results').count(), 0, 'empty Spotlight stays compact');
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('entryneedle');
  await until(
    async () => (await main.locator('.content-panel').getByRole('option').count()) === 100,
    'bounded text preview',
  );
  assert(!(await main.locator('.content-panel').innerText()).includes('entry-106.md'));
  await main.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
  let panel = main.locator('.chat-search-selection');
  await panel.getByText('검색 결과 파일 107/107개 선택', { exact: true }).waitFor();
  const full = (await invoke('rag_list', { query: 'entryneedle', limit: 100 }))[0];
  const last = await invoke('rag_file_set_page', { id: full.id, offset: 100 });
  assert.equal(last.files.length, 7);
  assert(last.files.some((file) => file.path.endsWith('entry-106.md')));
  assert.equal(calls.length, 0);
  await main.getByRole('textbox', { name: '질문', exact: true }).fill('unseenlastneedle');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(() => calls.length === 1, 'evidence outside preview reaches provider');
  assert(JSON.stringify(calls[0]).includes('unseenlastneedle'));
  assert(
    JSON.stringify(calls[0]).includes('TAIL_EVIDENCE_107'),
    'the last file contributed actual evidence',
  );
  assert(!JSON.stringify(calls[0]).includes('hiddenmetadata'));
  await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').waitFor();

  await invoke('show_quick');
  await quick.getByRole('textbox', { name: '검색어', exact: true }).fill('entry-');
  // The shared session is still Content; switch mode in the real session API.
  const session = await invoke('get_search_session');
  await invoke('set_search_session', {
    session: { ...session, mode: 'filename', query: 'entry-' },
  });
  await until(
    async () => (await quick.locator('.file-results').getByRole('option').count()) === 110,
    'Spotlight filename results including two root directories',
  );
  await quick.setViewportSize({ width: 390, height: 480 });
  await quick.screenshot({
    path: join(evidence, 'chat-entry-quick-mobile.png'),
    animations: 'disabled',
  });
  assert(await quick.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await quick.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
  await main
    .locator('.chat-search-selection')
    .getByText('검색 결과 파일 108/108개 선택', { exact: true })
    .waitFor();
  assert.equal(calls.length, 1, 'Spotlight only creates/opens the conversation');
  assert.equal(await invoke('rag_take_chat_handoff'), null, 'main consumed its queued handoff');

  // Unsaved document editing is not silently lost when another webview requests chat.
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main.getByRole('button', { name: '새 문서', exact: true }).click();
  await main.getByLabel('문서 제목', { exact: true }).fill('보존할 입력');
  await invoke('show_quick');
  const next = await invoke('get_search_session');
  await invoke('set_search_session', {
    session: { ...next, mode: 'content', query: 'entryneedle' },
  });
  await until(
    async () => (await quick.locator('.content-panel').getByRole('option').count()) === 100,
    'Spotlight text preview',
  );
  await quick.getByRole('button', { name: '검색 결과로 대화', exact: true }).waitFor();
  assert(await quick.getByRole('button', { name: '검색 결과로 대화', exact: true }).isEnabled());
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Control+Shift+Enter');
  await main
    .getByText(
      'Spotlight의 대화가 준비됐습니다. 작성 중인 문서를 저장하거나 편집을 버리면 이동합니다.',
      { exact: true },
    )
    .waitFor();
  assert.equal(await main.getByLabel('문서 제목', { exact: true }).inputValue(), '보존할 입력');
  await main.screenshot({ path: join(evidence, 'chat-entry-unsaved.png'), animations: 'disabled' });
  await main.getByRole('button', { name: '취소', exact: true }).click();
  await main
    .locator('.chat-search-selection')
    .getByText('검색 결과 파일 107/107개 선택', { exact: true })
    .waitFor();
  assert.equal(calls.length, 1);
  const unauthorized = await quick.evaluate(async () => {
    try {
      await window.__TAURI_INTERNALS__.invoke('rag_take_chat_handoff');
      return null;
    } catch (error) {
      return error.code;
    }
  });
  assert.equal(unauthorized, 'input');
  // Delay only the actual listen request, without replacing IPC responses or
  // event delivery. The wakeup is lost; startup must drain the queued ID.
  await main.addInitScript(() => {
    const original = window.fetch.bind(window);
    window.__chatListenHeld = false;
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (
        url.hostname === 'ipc.localhost' &&
        decodeURIComponent(url.pathname).includes('plugin:event|listen')
      ) {
        const raw = options?.body;
        const body = JSON.parse(typeof raw === 'string' ? raw : new TextDecoder().decode(raw));
        if (body.event === 'prism:chat-handoff') {
          window.__chatListenHeld = true;
          await new Promise((resolve) => {
            window.__releaseChatListen = resolve;
          });
        }
      }
      return original(input, options);
    };
  });
  try {
    await main.reload();
    await until(
      () => main.evaluate(() => window.__chatListenHeld),
      'actual chat listener registration held',
    );
    await quick.evaluate(
      (id) => window.__TAURI_INTERNALS__.invoke('rag_open_main_chat', { id }),
      full.id,
    );
  } finally {
    await main.evaluate(() => window.__releaseChatListen?.());
  }
  await main
    .locator('.chat-search-selection')
    .getByText('검색 결과 파일 107/107개 선택', { exact: true })
    .waitFor();
  await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').waitFor();
  assert.equal(await invoke('rag_take_chat_handoff'), null);
  assert.equal(calls.length, 1, 'late registration does not send another question');
  await main.screenshot({ path: join(evidence, 'chat-entry-main.png'), animations: 'disabled' });
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      contentFiles: 107,
      filenameFiles: 108,
      generation: 'synthetic loopback',
      externalCalls: 0,
    }),
  );
} catch (error) {
  console.error(JSON.stringify({ directory, calls: calls.length }));
  if (instance) {
    console.error((await instance.quick.locator('main').innerText()).slice(0, 1500));
    console.error(JSON.stringify(await instance.invoke('get_search_session')));
    const diagnostic = await instance.invoke('search_files', {
      query: 'entry-',
      extension: '',
      scopeId: null,
    });
    console.error(
      JSON.stringify({
        filenameTotal: diagnostic.total,
        status: diagnostic.status,
        errors: instance.errors,
      }),
    );
  }
  throw error;
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
