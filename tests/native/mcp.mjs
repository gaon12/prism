// Actual bundled app, SQLite, stdio MCP server and chat UI; only generation is
// a synthetic loopback HTTP provider. No credentials, external API or user data.
// Browser plugin not available: repository Playwright/WebView2 native harness.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launch, shutdown, until } from './harness.mjs';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';

assert.equal(process.platform, 'win32', 'Windows WebView2 integration check');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply an explicit candidate executable');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile));
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const root = await mkdtemp(join(tmpdir(), 'prism-mcp-native-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
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
const folder = join(root, 'documents');
await mkdir(folder);
const localCanary = 'LOCAL_DOCUMENT_BODY_CANARY_54829';
const externalCanary = 'EXTERNAL_RESULT_CANARY_75321';
await writeFile(
  join(folder, '자료.md'),
  `mcpneedle 로컬 외부 질문 후속 철회 후 테스트 자료 ${localCanary}`,
);
const trace = join(root, 'mcp-trace.jsonl');
const control = join(root, 'mcp-control.json');
await writeFile(trace, '');
await writeFile(control, JSON.stringify({ mode: 'normal', text: externalCanary }));
const descriptor = {
  id: 'native-fixture',
  name: '격리 MCP 서버',
  command: process.execPath,
  args: [fileURLToPath(new URL('./fixtures/mcp-server.mjs', import.meta.url)), trace, control],
  allowedTools: [],
  search: { tool: 'search', argument: 'query' },
};
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    mcp: { enabled: true, servers: [descriptor] },
    scopes: [
      {
        id: 'fixture',
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
  (await readFile(trace, 'utf8')).split(/\r?\n/).filter(Boolean).map(JSON.parse);
const calls = async () =>
  (await events()).filter((entry) => entry.message?.method === 'tools/call');
const starts = async () => (await events()).filter((entry) => entry.event === 'start');
const isRunning = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === 'ESRCH') return false;
    throw error;
  }
};
const requests = [];
const sockets = new Set();
let answer = '로컬답변표식 [S1]';
const provider = createServer((request, response) => {
  const parts = [];
  request.on('data', (part) => parts.push(part));
  request.on('end', () => {
    requests.push(JSON.parse(Buffer.concat(parts).toString('utf8')));
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({ choices: [{ message: { content: answer }, finish_reason: 'stop' }] }),
    );
  });
});
provider.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => provider.listen(0, '127.0.0.1', done));
let instance;
const screenshots = [];
const resultPath = process.env.PRISM_NATIVE_RESULT ?? join(root, 'result.json');
let summary;
try {
  instance = await launch(data, { executable, portable: true });
  const { invoke, main } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.development, profile === '0');
  assert.equal(runtime.installKind, 'portable');
  const save = async (transform) => {
    const current = await invoke('get_settings');
    return invoke('save_settings', {
      settings: transform(current),
      expectedRevision: current.revision,
    });
  };
  const page = (id) => invoke('rag_conversation', { id, beforeSequence: null, limit: 100 });
  await until(async () => {
    const s = await invoke('content_status');
    return s.phase === 'ready' && s.indexed === 1;
  }, 'fixture content index');
  await save((settings) => ({
    ...settings,
    generation: {
      ...settings.generation,
      retrieval: 'keyword',
      local: {
        endpoint: `http://127.0.0.1:${provider.address().port}/v1`,
        model: 'synthetic-native',
        maxTokens: 256,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
  }));
  assert.equal((await starts()).length, 0, 'Saving MCP config must not launch it automatically');
  const tools = await invoke('mcp_probe', { serverId: descriptor.id });
  assert.equal(tools.find((tool) => tool.name === 'search').usable, true);
  assert.equal(tools.find((tool) => tool.name === 'delete').usable, false);
  await until(
    async () => (await starts()).every((entry) => !isRunning(entry.pid)),
    'probe process disposed',
  );
  assert.equal((await calls()).length, 0, 'Probe only lists tools');
  const probeStarts = (await starts()).length;
  await main.getByRole('button', { name: '채팅', exact: true }).click();
  await main.getByRole('heading', { name: '문서와 대화', exact: true }).waitFor();
  const beforeCreate = new Set(
    (await invoke('rag_list', { query: '', limit: 100, project: null })).map((item) => item.id),
  );
  await main.getByRole('button', { name: '새 대화', exact: true }).click();
  let created;
  await until(async () => {
    const conversations = await invoke('rag_list', { query: '', limit: 100, project: null });
    created = conversations.find((item) => !beforeCreate.has(item.id));
    return created;
  }, 'new UI conversation');
  const id = created.id;
  const question = main.getByRole('textbox', { name: '질문', exact: true });
  const optIn = main.getByRole('checkbox', { name: '외부 자료도 찾기', exact: true });
  assert.equal(await optIn.isChecked(), false);
  await question.fill('mcpneedle 로컬 질문');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () => (await page(id)).turns.some((turn) => turn.text.includes('로컬답변표식')),
    'local-only chat answer',
  );
  assert.equal((await starts()).length, probeStarts, 'Question opt-out must not start a server');
  assert.equal((await calls()).length, 0);
  assert(!JSON.stringify(requests.at(-1)).includes(externalCanary));
  const externalQuestion = 'mcpneedle 외부 질문';
  answer = '외부답변표식 [E1] [S1]';
  await question.fill(externalQuestion);
  await optIn.check();
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () => (await page(id)).turns.some((turn) => turn.text.includes('외부답변표식')),
    'actual MCP answer',
  );
  const called = await calls();
  assert.equal(called.length, 1);
  assert.equal(called[0].message.params.name, 'search');
  assert.deepEqual(called[0].message.params.arguments, { query: externalQuestion });
  assert(
    !JSON.stringify(await events()).includes(localCanary),
    'MCP receives no local document excerpts',
  );
  assert(
    !JSON.stringify(await events()).includes('로컬답변표식'),
    'MCP receives no conversation history',
  );
  await until(
    async () => (await starts()).every((entry) => !isRunning(entry.pid)),
    'search process disposed',
  );
  const externalTurn = (await page(id)).turns.find((turn) => turn.text.includes('외부답변표식'));
  assert.equal(externalTurn.external[0].outcome, 'ok');
  assert.equal(externalTurn.external[0].text, externalCanary);
  assert(externalTurn.citations.some((citation) => citation.label === 'E1' && citation.isCited));
  assert(externalTurn.provenance.some((source) => source.scopeId.startsWith('mcp_')));
  const sent = JSON.stringify(requests.at(-1));
  assert(sent.includes(externalCanary) && sent.includes('[E1] external search result'));
  assert(sent.includes(localCanary), 'Local retrieval still reaches the chosen local provider');
  await main.locator('.chat-external').getByText('격리 MCP 서버', { exact: true }).waitFor();
  assert.equal(await optIn.isChecked(), false, 'External opt-in resets for each question');
  await main.locator('.chat-external details > summary').click();
  await main.locator('.chat-external').getByText(externalCanary, { exact: true }).waitFor();
  screenshots.push(join(root, 'mcp-answer.png'));
  await main.screenshot({ path: screenshots.at(-1) });
  answer = '후속답변표식 [S1]';
  await question.fill('mcpneedle 후속 질문');
  await main.getByRole('button', { name: '질문 전송', exact: true }).click();
  await until(
    async () => (await page(id)).turns.some((turn) => turn.text.includes('후속답변표식')),
    'inherited provenance answer',
  );
  const inherited = (await page(id)).turns.find((turn) => turn.text.includes('후속답변표식'));
  assert(inherited.provenance.some((source) => source.scopeId.startsWith('mcp_')));
  assert.equal((await calls()).length, 1, 'A follow-up does not silently repeat external search');
  await until(
    async () =>
      (await main.locator('.chat-turn[data-role="assistant"][data-status="complete"]').count()) ===
        3 && (await optIn.isEnabled()),
    'completed follow-up rendered',
  );
  await save((settings) => ({ ...settings, mcp: { enabled: true, servers: [] } }));
  const revoked = await page(id);
  for (const turn of revoked.turns.filter((turn) =>
    [externalTurn.id, inherited.id].includes(turn.id),
  )) {
    assert.equal(turn.externalRevoked, true);
    assert.equal(turn.text, '');
    assert(turn.external.every((item) => item.text === ''));
  }
  const exported = await invoke('rag_export', { id, includeSources: true });
  for (const token of [externalCanary, '외부답변표식', '후속답변표식']) {
    assert(!exported.includes(token));
    assert.equal((await invoke('rag_list', { query: token, limit: 100, project: null })).length, 0);
  }
  const revokedMessage =
    '이 답변은 지금은 허용되지 않는 외부 자료를 사용해서 숨겼습니다. 설정에서 해당 서버를 그대로 되돌리면 다시 보입니다.';
  for (const turnId of [externalTurn.id, inherited.id]) {
    const notice = main
      .locator('.chat-turn')
      .filter({ has: main.locator(`[id="turn-${turnId}"]`) })
      .getByText(revokedMessage, { exact: true });
    await notice.waitFor();
    await notice.scrollIntoViewIfNeeded();
  }
  const rendered = await main.locator('.chat-transcript').innerText();
  for (const token of [externalCanary, '외부답변표식', '후속답변표식'])
    assert(!rendered.includes(token));
  screenshots.push(join(root, 'mcp-revoked.png'));
  await main.screenshot({ path: screenshots.at(-1) });
  const previousRequests = requests.length;
  answer = '철회후답변표식 [S1]';
  await invoke('rag_ask', {
    request: {
      requestId: crypto.randomUUID(),
      conversationId: id,
      question: 'mcpneedle 철회 후',
      scopeIds: ['fixture'],
      mode: 'documents',
      sourcesHidden: false,
      external: false,
    },
  });
  assert.equal(requests.length, previousRequests + 1);
  for (const token of [externalCanary, '외부답변표식', '후속답변표식'])
    assert(!JSON.stringify(requests.at(-1)).includes(token));
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(data, { executable, portable: true });
  const afterRestart = await instance.invoke('rag_conversation', {
    id,
    beforeSequence: null,
    limit: 100,
  });
  assert(afterRestart.turns.find((turn) => turn.id === externalTurn.id).externalRevoked);
  assert(!JSON.stringify(afterRestart).includes(externalCanary));
  const current = await instance.invoke('get_settings');
  await instance.invoke('save_settings', {
    settings: { ...current, mcp: { enabled: true, servers: [descriptor] } },
    expectedRevision: current.revision,
  });
  const restored = await instance.invoke('rag_conversation', {
    id,
    beforeSequence: null,
    limit: 100,
  });
  assert.equal(
    restored.turns.find((turn) => turn.id === externalTurn.id).text,
    '외부답변표식 [E1] [S1]',
  );
  assert.equal(restored.turns.find((turn) => turn.id === inherited.id).text, '후속답변표식 [S1]');
  // Block a real stdio call, then cancel before any generation request starts.
  await writeFile(control, JSON.stringify({ mode: 'stall', text: externalCanary }));
  const cancelConversation = await instance.invoke('rag_create', { title: 'MCP 취소 검증' });
  const cancelRequest = {
    requestId: crypto.randomUUID(),
    conversationId: cancelConversation.id,
    question: 'mcpneedle 질문',
    scopeIds: ['fixture'],
    mode: 'documents',
    sourcesHidden: false,
    external: true,
  };
  const beforeCalls = (await calls()).length;
  const beforeGeneration = requests.length;
  const pending = instance.main.evaluate(async (request) => {
    try {
      return { value: await window.__TAURI_INTERNALS__.invoke('rag_ask', { request }) };
    } catch (error) {
      return { error: { code: error?.code, message: error?.message } };
    }
  }, cancelRequest);
  await until(async () => (await calls()).length === beforeCalls + 1, 'blocked MCP tool call');
  const cancelledAt = Date.now();
  await instance.invoke('rag_cancel', {
    conversationId: cancelConversation.id,
    requestId: cancelRequest.requestId,
  });
  assert.equal((await pending).error?.code, 'cancelled');
  const cancellationMs = Date.now() - cancelledAt;
  assert(cancellationMs < 3000, `Cancellation took ${cancellationMs} ms`);
  await until(
    async () => (await starts()).every((entry) => !isRunning(entry.pid)),
    'cancelled server disposed',
  );
  assert.equal(requests.length, beforeGeneration, 'Cancelled search must not reach generation');
  assert.deepEqual(instance.errors, []);
  summary = {
    passed: true,
    root,
    executableSha256,
    testSha256: await hash(fileURLToPath(import.meta.url)),
    fixtureSha256: await hash(fileURLToPath(new URL('./fixtures/mcp-server.mjs', import.meta.url))),
    development: profile === '0',
    generation: 'synthetic loopback provider',
    server: 'actual Node MCP stdio process',
    questionOnly: true,
    optOutStarts: 0,
    toolCalls: (await calls()).length,
    revokeUiExportSearchHistory: true,
    restartRevocation: true,
    exactServerRestore: true,
    cancellationMs,
    cancelledBeforeGeneration: true,
    screenshots,
    errors: [],
  };
  console.log(JSON.stringify(summary));
} catch (error) {
  if (instance) {
    await instance.main.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
    await writeFile(
      join(root, 'failure-ui.txt'),
      await instance.main.locator('body').innerText(),
    ).catch(() => {});
  }
  await writeFile(join(root, 'provider-requests.json'), JSON.stringify(requests, null, 2));
  throw error;
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => provider.close(done));
  await writeFile(
    resultPath,
    JSON.stringify(summary ?? { passed: false, root, executableSha256 }, null, 2),
  );
}
