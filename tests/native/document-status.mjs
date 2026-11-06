// Actual own/foreign document completion polling with original IPC responses held. Only generation is synthetic
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
    assert.equal(input.operation, 'outline');
    const payload = {
      title: 'researchneedle',
      sections: [{ id: 'summary', heading: '조사 결과', instruction: 'researchneedle' }],
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
let releaseGates = async () => {};
let diagnose = async () => ({});
try {
  instance = await launch(data, { executable, portable: true });
  const { main, invoke } = instance;
  await main.evaluate(() => {
    const gate = (window.__ownJobRefresh = {
      initial: false,
      ready: false,
      holdReads: false,
      eligible: 0,
      jobs: [],
      reads: [],
    });
    let earlyRelease, terminalRelease, readsRelease;
    const early = new Promise((done) => {
      earlyRelease = done;
    });
    const terminal = new Promise((done) => {
      terminalRelease = done;
    });
    const reads = new Promise((done) => {
      readsRelease = done;
    });
    gate.releaseEarly = earlyRelease;
    gate.releaseTerminal = terminalRelease;
    gate.releaseReads = readsRelease;
    const original = window.fetch.bind(window);
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      const isJob = url.hostname === 'ipc.localhost' && url.pathname.endsWith('/doc_job');
      const isRead = url.hostname === 'ipc.localhost' && url.pathname.endsWith('/doc_get');
      const afterReady = gate.ready;
      const response = await original(input, options);
      if (isJob) {
        const actual = await response.clone().json();
        gate.jobs.push({
          afterReady,
          phase: actual?.phase ?? null,
          requestId: actual?.requestId ?? null,
        });
        if (!gate.initial) {
          const assertInitial = actual === null;
          gate.initial = true;
          gate.initialEmpty = assertInitial;
        } else if (!afterReady) await early;
        else {
          gate.eligible++;
          await terminal;
        }
      }
      if (isRead) {
        gate.reads.push({ held: gate.holdReads });
        if (gate.holdReads) await reads;
      }
      return response;
    };
  });
  releaseGates = () =>
    main.evaluate(() => {
      const gate = window.__ownJobRefresh;
      gate.holdReads = false;
      gate.releaseEarly();
      gate.releaseTerminal();
      gate.releaseReads();
    });
  const outlineRequests = [];
  const outlineResponses = [];
  main.on('request', (request) => {
    if (request.url().includes('/doc_generate_outline')) {
      outlineRequests.push({ url: request.url(), body: request.postData() });
    }
  });
  main.on('response', (response) => {
    if (!response.url().includes('/doc_generate_outline')) return;
    const record = { status: response.status(), body: null, readError: null };
    outlineResponses.push(record);
    void response.text().then(
      (body) => {
        record.body = body.slice(0, 1600);
      },
      (error) => {
        record.readError = String(error);
      },
    );
  });
  await main.evaluate(() => {
    window.__documentMcpClicks = [];
    for (const type of ['pointerdown', 'pointerup', 'click'])
      document.addEventListener(
        type,
        (event) => {
          const button = event.target.closest?.('.doc-actions button');
          if (button)
            window.__documentMcpClicks.push({
              type,
              time: performance.now(),
              label: button.textContent,
              trusted: event.isTrusted,
              disabled: button.disabled,
              focused: document.hasFocus(),
              options: [...document.querySelectorAll('.doc-options input')].map((input) => ({
                type: input.type,
                value: input.value,
                checked: input.checked,
              })),
            });
        },
        true,
      );
  });
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.development, profile === '0');
  assert.equal(runtime.installKind, 'portable');
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 1;
  }, 'local fixture indexed');
  const created = await invoke('doc_create', {
    input: {
      title: '격리 MCP 문서',
      purpose: 'researchneedle',
      templateMarkdown: templateCanary,
      scopeIds: ['docs'],
    },
  });
  const current = () => invoke('doc_get', { id: created.document.id, revisionId: null });
  diagnose = async () => ({
    outlineRequests,
    outlineResponses,
    clicks: await main.evaluate(() => window.__documentMcpClicks),
    job: await invoke('doc_job', { id: created.document.id }).catch(() => null),
    alerts: await main.locator('[role="alert"]').allTextContents(),
    buttons: await main
      .locator('.doc-actions button')
      .evaluateAll((items) =>
        items.map((item) => ({ text: item.textContent, disabled: item.disabled })),
      ),
    events: await events(),
    errors: instance.errors,
  });
  await main.getByRole('button', { name: '문서', exact: true }).click();
  await main
    .locator('.doc-sidebar')
    .getByRole('button')
    .filter({ hasText: '격리 MCP 문서' })
    .click();
  const outline = main.getByRole('button', { name: 'AI로 개요 생성', exact: true });
  await until(() => outline.isEnabled(), 'outline action ready');
  await until(
    () => main.evaluate(() => window.__ownJobRefresh.initial),
    'initial actual no-job status',
  );
  assert(await main.evaluate(() => window.__ownJobRefresh.initialEmpty));
  await outline.click();
  await until(
    async () => (await current()).revision.operation === 'outline',
    'outline without research',
  );
  assert.equal((await events()).length, 0, 'no opt-in means no server process or tool request');
  await until(() => outline.isEnabled(), 'own generation response applied');
  await until(
    async () => (await main.locator('.doc-editor-heading small').textContent()) === '버전 2',
    'own revision displayed',
  );
  await main.evaluate(() => {
    window.__ownJobRefresh.ready = true;
    window.__ownJobRefresh.holdReads = true;
    window.__ownJobRefresh.reads = [];
  });
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
  await main.evaluate(() => window.__ownJobRefresh.releaseEarly());
  await until(
    () => main.evaluate(() => window.__ownJobRefresh.eligible > 0),
    'post-completion terminal status held',
  );
  await main.evaluate(() => window.__ownJobRefresh.releaseTerminal());
  await until(
    () => main.evaluate(() => window.__ownJobRefresh.eligible >= 2),
    'terminal status processed before next poll',
  );
  const ownBoundary = await main.evaluate(() => ({
    jobs: window.__ownJobRefresh.jobs,
    reads: window.__ownJobRefresh.reads,
    outlineEnabled:
      [...document.querySelectorAll('.doc-actions button')].find(
        (button) => button.textContent === 'AI로 개요 생성',
      )?.disabled === false,
  }));
  await writeFile(join(root, 'own-completion-boundary.json'), JSON.stringify(ownBoundary, null, 2));
  await main.screenshot({ path: join(root, 'own-completion-boundary.png') });
  assert.equal(
    ownBoundary.outlineEnabled,
    true,
    'a completed own request must not disable a ready generation action',
  );
  assert.equal(
    ownBoundary.reads.length,
    0,
    'the already displayed own completion needs no document reload',
  );
  await releaseGates();
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
  await until(() => outline.isEnabled(), 'second own response applied');
  const own = await current();
  await main.evaluate(() => {
    window.__ownJobRefresh.reads = [];
  });
  const external = await invoke('doc_generate_outline', {
    request: {
      documentId: created.document.id,
      expectedRevisionId: own.revision.id,
      requestId: crypto.randomUUID(),
      instruction: '',
      scopeIds: ['docs'],
      researchQuery: null,
    },
  });
  await until(
    async () =>
      (await main.locator('.doc-editor-heading small').textContent()) ===
      `버전 ${external.sequence}`,
    'another caller completion still refreshes the editor',
  );
  const unknownReads = await main.evaluate(() => window.__ownJobRefresh.reads.length);
  assert(unknownReads > 0, 'an unseen request still fetches its actual revision');
  const starts = (await events()).filter((event) => event.event === 'start');
  await until(
    () => starts.every((event) => !alive(event.pid)),
    'owned MCP fixture processes reaped',
  );
  assert.equal(await hash(source), sourceSha256);
  assert.deepEqual(instance.errors, []);
  const result = {
    passed: true,
    root,
    executableSha256,
    release: profile === '1',
    ownBoundary,
    unknownReads,
    diagnostic: await diagnose(),
  };
  await writeFile(join(root, 'own-refresh-proof.json'), JSON.stringify(result, null, 2));
  console.log(
    JSON.stringify({
      passed: true,
      root,
      executableSha256,
      ownCompletionReads: ownBoundary.reads.length,
      unknownCompletionReads: unknownReads,
      mcpCalls: (await calls()).length,
      generationRequests: requests.length,
      errors: instance.errors,
    }),
  );
} catch (error) {
  await releaseGates().catch(() => {});
  const diagnostic = await diagnose().catch((cause) => ({ diagnosticError: String(cause) }));
  await writeFile(join(root, 'failure.json'), JSON.stringify(diagnostic, null, 2));
  if (instance) await instance.main.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
  console.log(JSON.stringify({ root, failed: true, ...diagnostic }));
  throw error;
} finally {
  await releaseGates().catch(() => {});
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => provider.close(done));
}
