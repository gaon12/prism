// Real filename/content/RAG/SQLite/UI flow, synthetic loopback generation only.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
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
  Array.from({ length: 1005 }, (_, i) => writeFile(join(names, `표본-${i}.md`), 'not indexed')),
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
  await main.evaluate(() => {
    window.__searchChatActions = [];
    document.addEventListener(
      'click',
      (event) => {
        const button = event.target.closest?.('button');
        if (button)
          window.__searchChatActions.push({
            button: button.textContent,
            disabled: button.disabled,
            question: document.querySelector('.chat-composer textarea')?.value,
          });
      },
      true,
    );
  });
  assert.equal((await invoke('runtime_info')).shortcutError, null);
  const startModel = async () => {
    await invoke('model_start');
    await until(
      async () => (await invoke('model_status')).phase === 'running',
      'real embedding runtime starts',
      120_000,
    );
    await until(
      async () => {
        const status = await invoke('content_status');
        return (
          status.phase === 'ready' && status.embedded === status.indexed && !status.embeddingError
        );
      },
      'actual selected documents embedded',
      120_000,
    );
  };
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'only permitted content indexed');
  await until(
    async () => !(await invoke('index_status')).scanning,
    'filename catalog snapshot settled',
  );
  if (process.env.PRISM_TEST_SEARCH_CHAT_MODEL === '1') {
    await invoke('model_import', {
      path: resolve('.dev/models/embeddinggemma-2-Q8_0.gguf'),
      runtime: false,
    });
    await invoke('model_import', {
      path: resolve('.dev/runtime/llama-b11960-bin-win-cpu-x64.zip'),
      runtime: true,
    });
    await startModel();
  }
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
  // Delay only the real read response: background transcript refresh must not
  // disable a ready question or replay an older page over a saved file choice.
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__readGates = [];
    window.__holdConversation = true;
    window.fetch = async (input, options) => {
      const response = await original(input, options);
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (
        window.__holdConversation &&
        url.hostname === 'ipc.localhost' &&
        url.pathname.endsWith('/rag_conversation')
      ) {
        await new Promise((resolve) => window.__readGates.push(resolve));
      }
      return response;
    };
  });
  await main
    .locator('.chat-workspace')
    .getByRole('button', { name: '새로고침', exact: true })
    .click();
  await until(
    async () => main.evaluate(() => window.__readGates.length > 0),
    'actual transcript read held',
  );
  assert(
    await main.getByRole('button', { name: '질문 전송', exact: true }).isEnabled(),
    'Read-only refresh does not interrupt a ready question',
  );
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    () => requests.length === 2,
    'question reaches provider while the old transcript read is held',
  );
  await main.evaluate(() => {
    window.__holdConversation = false;
    for (const release of window.__readGates) release();
    window.__readGates = [];
  });
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
      .selectionSet.selectedCount,
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
  assert.equal(persisted.selectionSet.selectedCount, 0);
  const lateFolder = join(directory, 'z-late');
  await mkdir(lateFolder);
  await writeFile(
    join(lateFolder, '표본-zz-tail.md'),
    'lateselectionneedle selected evidence beyond the displayed results',
  );
  const settings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: {
      ...settings,
      scopes: [
        ...settings.scopes,
        {
          id: 'late',
          name: '후반 파일',
          path: lateFolder,
          filenameEnabled: true,
          contentEnabled: true,
          externalAllowed: false,
        },
      ],
    },
    expectedRevision: settings.revision,
  });
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 3;
  }, 'late authorized content indexed without reading filename-only files');
  if (process.env.PRISM_TEST_SEARCH_CHAT_MODEL === '1') await startModel();
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('표본');
  await until(
    async () => (await main.getByRole('option').count()) === 200,
    'bounded filename result set',
  );
  assert.equal(
    await main.getByRole('option').filter({ hasText: '표본-zz-tail.md' }).count(),
    0,
    'late source is outside displayed results',
  );
  await main.getByRole('button', { name: '검색 결과로 대화', exact: true }).click();
  const partial = main.locator('.chat-search-selection[data-paged="true"]');
  await partial.getByText('검색 결과 파일 1006/1006개 선택', { exact: true }).waitFor();
  const fullConversation = (await invoke('rag_list', { query: '표본', limit: 100 }))[0];
  await main.getByRole('textbox', { name: '질문', exact: true }).fill('lateselectionneedle');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () =>
      requests.length === 3 &&
      (await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').count()) ===
        1,
    'file beyond UI and first 256 targets is retrieved',
    60_000,
  );
  assert(JSON.stringify(requests[2]).includes('lateselectionneedle selected evidence'));
  assert(!JSON.stringify(requests[2]).includes('privatecanary'));
  assert(!JSON.stringify(requests[2]).includes('approvedcontext'));
  assert(!JSON.stringify(requests[2]).includes(directory));
  await partial.locator('summary').click();
  assert.equal(await partial.getByRole('checkbox').count(), 50);
  for (let page = 1; page <= 20; page++) {
    await partial.getByRole('button', { name: '다음 파일', exact: true }).click();
    await partial
      .getByText(`1006개 중 ${page * 50 + 1}–${Math.min(page * 50 + 50, 1006)}`, { exact: true })
      .waitFor();
  }
  assert.equal(await partial.getByRole('checkbox').count(), 6);
  await partial.getByRole('checkbox', { name: /표본-zz-tail.md/ }).uncheck();
  await partial.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
  await partial.getByText('검색 결과 파일 1005/1006개 선택', { exact: true }).waitFor();
  assert.equal(
    (await invoke('rag_file_set_page', { id: fullConversation.id, offset: 1000 })).selectedCount,
    1005,
  );
  const stale = await main.evaluate(async (id) => {
    try {
      await window.__TAURI_INTERNALS__.invoke('rag_edit_file_set', {
        id,
        revision: 1,
        changes: [],
        all: false,
        clear: false,
      });
      return 'unexpected';
    } catch (error) {
      return error.code;
    }
  }, fullConversation.id);
  assert.equal(stale, 'policyChanged');
  await partial.getByRole('button', { name: '파일 선택 해제', exact: true }).click();
  await partial.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
  await partial.getByText('검색 결과 파일 0/1006개 선택', { exact: true }).waitFor();
  assert.equal(
    (await invoke('rag_file_set_page', { id: fullConversation.id, offset: 0 })).selectedCount,
    0,
  );
  await partial.getByRole('button', { name: '파일 전체 선택', exact: true }).click();
  await partial.getByRole('button', { name: '파일 선택 저장', exact: true }).click();
  await partial.getByText('검색 결과 파일 1006/1006개 선택', { exact: true }).waitFor();
  await main.setViewportSize({ width: 390, height: 844 });
  await partial.locator('summary').scrollIntoViewIfNeeded();
  await partial.evaluate((element) => element.scrollIntoView({ block: 'start' }));
  await main.screenshot({ path: join(evidence, 'search-chat-mobile.png'), animations: 'disabled' });
  assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  assert.deepEqual(instance.errors, []);
  const finalModel = await invoke('model_status');
  if (process.env.PRISM_TEST_SEARCH_CHAT_MODEL === '1') {
    assert.equal(finalModel.phase, 'running');
    assert.equal(finalModel.modelId, 'google/embeddinggemma-2');
  }
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      generation: 'synthetic loopback',
      retrieval:
        process.env.PRISM_TEST_SEARCH_CHAT_MODEL === '1'
          ? 'actual local EmbeddingGemma 2'
          : 'keywords',
      model: finalModel.modelReady ? finalModel.modelId : null,
      snapshotFiles: 1006,
      externalCalls: 0,
    }),
  );
} catch (error) {
  console.error(JSON.stringify({ directory, requests: requests.length }));
  if (instance) {
    try {
      console.error(JSON.stringify(await instance.main.evaluate(() => window.__searchChatActions)));
    } catch {}
    try {
      console.error(await instance.main.locator('.chat-workspace').innerText());
    } catch {}
    try {
      console.error(JSON.stringify(await instance.invoke('rag_list', { query: '', limit: 100 })));
    } catch {}
  }
  throw error;
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
