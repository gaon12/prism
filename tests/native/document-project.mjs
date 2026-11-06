// Actual project/session, indexing, document IPC and editor state. Only the
// loopback generation response is synthetic; source membership is real.
// Browser plugin not available: repository Playwright/WebView2 harness.
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
const root = await mkdtemp(join(tmpdir(), 'prism-document-project-'));
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const executable = join(root, 'prism.exe');
const data = join(root, 'PrismData');
await mkdir(data);
await copyFile(original, executable);
writePackageNotices(root, loadPackageNotices(original));
await writeFile(join(root, 'prism.portable'), '');
const hash = async (path) => {
  const digest = createHash('sha256');
  for await (const bytes of createReadStream(path)) digest.update(bytes);
  return digest.digest('hex');
};
const executableSha256 = await hash(executable);
assert.equal(await hash(original), executableSha256);
const scopes = [];
const sources = [];
for (const [id, name, contentEnabled] of [
  ['alpha', '프로젝트 알파', true],
  ['beta', '프로젝트 베타', true],
  ['names', '파일명 전용', false],
]) {
  const path = join(root, id);
  await mkdir(path);
  const file = join(path, `${id}.md`);
  await writeFile(file, `projectneedle SOURCE_${id.toUpperCase()}_PRIVATE_CANARY`);
  sources.push({ file, sha256: await hash(file) });
  scopes.push({ id, name, path, contentEnabled, filenameEnabled: true, externalAllowed: false });
}
const requests = [];
const sockets = new Set();
const provider = createServer((request, response) => {
  const bytes = [];
  request.on('data', (chunk) => bytes.push(chunk));
  request.on('end', () => {
    const packet = JSON.parse(Buffer.concat(bytes).toString());
    requests.push(packet);
    const prompt = packet.messages.at(-1).content;
    const marker = 'Document input:\n';
    const input = JSON.parse(
      prompt.slice(prompt.lastIndexOf(marker) + marker.length).split('\n')[0],
    );
    const result =
      input.operation === 'outline'
        ? {
            title: 'projectneedle',
            sections: [{ id: 'summary', heading: '요약', instruction: 'projectneedle' }],
          }
        : {
            blocks: [
              {
                id: 'facts',
                sectionId: 'summary',
                kind: 'fact',
                markdown: '검토할 내용 [S1]',
                sourceLabels: ['[S1]'],
              },
            ],
          };
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({
        choices: [{ message: { content: JSON.stringify(result) }, finish_reason: 'stop' }],
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
    scopes,
    generation: {
      retrieval: 'keyword',
      local: {
        endpoint: `http://127.0.0.1:${provider.address().port}/v1`,
        model: 'synthetic-project-documents',
        maxTokens: 1024,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
  }),
);
let instance;
try {
  instance = await launch(data, { executable, portable: true });
  const { main, invoke } = instance;
  assert.equal((await invoke('runtime_info')).development, profile === '0');
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'two actual content scopes indexed');
  const setProject = async (scopeId) => {
    // The production command intentionally does not echo a debounced session
    // write into its originating window. Send from Spotlight to exercise the
    // actual cross-window update without replacing any client state.
    await instance.quick.evaluate(
      async (session) => {
        await window.__TAURI_INTERNALS__.invoke('set_search_session', { session });
      },
      { ...(await invoke('get_search_session')), scopeId },
    );
  };
  const chooseSidebarProject = async (name) => {
    await main
      .locator('.sidebar .folders-section button.rail-item')
      .filter({ hasText: name })
      .click();
    await main.getByRole('button', { name: '문서', exact: true }).click();
  };
  const selected = () => main.locator('.doc-scopes input:checked').count();
  await chooseSidebarProject('프로젝트 알파');
  await main.locator('.doc-sidebar').getByRole('button', { name: '새 문서', exact: true }).click();
  await main.getByLabel('문서 제목', { exact: true }).fill('알파 문서');
  await main.getByLabel('작성 목적·문서 유형', { exact: true }).fill('projectneedle');
  assert.equal(await selected(), 1);
  await main
    .locator('.document-workspace .doc-project')
    .filter({ hasText: '프로젝트 알파' })
    .waitFor();
  assert.equal(
    await main
      .locator('.doc-scopes')
      .innerText()
      .then((text) => text.includes('프로젝트 베타')),
    false,
  );
  await main.getByRole('button', { name: '초안 만들기', exact: true }).click();
  await main.locator('.doc-editor h2').filter({ hasText: '알파 문서' }).waitFor();
  const summary = (await invoke('doc_list', { query: '알파 문서', limit: 100 }))[0];
  const current = () => invoke('doc_get', { id: summary.id, revisionId: null });
  assert.deepEqual((await current()).revision.scopeIds, ['alpha']);
  const outline = main.getByRole('button', { name: 'AI로 개요 생성', exact: true });
  await until(() => outline.isEnabled(), 'project outline enabled');
  await outline.click();
  await main.getByLabel('항목 제목', { exact: true }).waitFor();
  assert.deepEqual((await current()).revision.scopeIds, ['alpha']);
  assert((await current()).revision.provenance.every((ref) => ref.scopeId === 'alpha'));
  assert(JSON.stringify(requests).includes('SOURCE_ALPHA_PRIVATE_CANARY'));
  assert(!JSON.stringify(requests).includes('SOURCE_BETA_PRIVATE_CANARY'));
  await main.getByLabel('개요를 검토했습니다.', { exact: true }).check();
  await main.getByRole('button', { name: '검토한 개요 저장', exact: true }).click();
  const body = main.getByRole('button', { name: '검토한 개요로 본문 생성', exact: true });
  await until(() => body.isEnabled(), 'reviewed project body enabled');
  await body.click();
  const edit = main.getByLabel('블록 Markdown', { exact: true });
  await edit.waitFor();
  const saved = await current();
  const draft = '프로젝트를 바꿔도 유지할 미저장 편집 [S1]';
  await edit.fill(draft);
  await main
    .locator('.sidebar .folders-section button.rail-item')
    .filter({ hasText: '프로젝트 베타' })
    .click();
  await main
    .locator('.document-workspace .doc-project')
    .filter({ hasText: '프로젝트 베타' })
    .waitFor();
  assert.equal(await edit.inputValue(), draft, 'sidebar project change retains dirty editor');
  assert.equal(
    (await current()).revision.id,
    saved.revision.id,
    'navigation does not silently save or adopt the revision',
  );
  const options = main.locator('.doc-options');
  if ((await options.getAttribute('open')) === null) await options.locator('summary').click();
  const scopePicker = main.locator('.doc-scopes');
  assert.equal(await scopePicker.locator('input:checked').count(), 1);
  assert((await scopePicker.innerText()).includes('프로젝트 베타'));
  assert(!(await scopePicker.innerText()).includes('프로젝트 알파'));
  const beta = scopePicker.locator('input');
  await beta.uncheck();
  await setProject('names');
  await main
    .locator('.document-workspace .doc-project')
    .filter({ hasText: '본문 검색이 꺼져' })
    .waitFor();
  assert.equal(await scopePicker.locator('input').count(), 0);
  assert.equal(await edit.inputValue(), draft);
  await setProject('unknown-project');
  await main.getByText('프로젝트 폴더가 제거되었습니다.', { exact: false }).waitFor();
  assert.equal(
    await scopePicker.locator('input').count(),
    0,
    'unknown project never widens to all folders',
  );
  await setProject('alpha');
  await main
    .locator('.document-workspace .doc-project')
    .filter({ hasText: '프로젝트 알파' })
    .waitFor();
  assert.equal(
    await scopePicker.locator('input:checked').count(),
    1,
    'new project gets its own defaults',
  );
  await main.screenshot({ path: join(root, 'document-project-dirty.png'), animations: 'disabled' });
  await main.getByRole('button', { name: '본문 편집 저장', exact: true }).click();
  await until(
    async () => (await current()).revision.blocks[0].markdown === draft,
    'actual dirty save',
  );
  await setProject('beta');
  const before = requests.length;
  const denied = await main.evaluate(
    async (request) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('doc_generate_body', { request });
        return null;
      } catch (error) {
        return { code: error.code, rag: error.rag?.code };
      }
    },
    {
      documentId: summary.id,
      expectedRevisionId: (await current()).revision.id,
      requestId: crypto.randomUUID(),
      instruction: '',
      scopeIds: ['beta'],
    },
  );
  assert.equal(
    denied?.rag,
    'sourceChanged',
    'old project provenance cannot be silently reused inside another project',
  );
  assert.equal(requests.length, before);
  await setProject(null);
  await until(
    async () => (await scopePicker.locator('input:checked').count()) === 2,
    'All files restores all content defaults',
  );
  await scopePicker.locator('input').first().uncheck();
  const settings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...settings, theme: 'dark' },
    expectedRevision: settings.revision,
  });
  assert.equal(
    await scopePicker.locator('input:checked').count(),
    1,
    'same-project settings refresh preserves explicit opt-out',
  );
  // Hold one actual document read, without fabricating its saved revision, to
  // prove that an old project's response cannot overwrite the new defaults.
  const alternate = await invoke('doc_create', {
    input: {
      title: '지연 조회 확인용',
      purpose: 'projectneedle',
      scopeIds: ['beta'],
      templateMarkdown: null,
    },
  });
  await main.locator('.doc-sidebar header button').click();
  await main.locator('.doc-sidebar button').filter({ hasText: '지연 조회 확인용' }).click();
  await main.locator('.doc-editor h2').filter({ hasText: alternate.document.title }).waitFor();
  await setProject('alpha');
  await main.evaluate((documentId) => {
    const original = window.fetch.bind(window);
    window.__documentProjectHeld = null;
    window.fetch = async (input, options) => {
      const response = await original(input, options);
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (
        url.hostname === 'ipc.localhost' &&
        url.pathname.endsWith('/doc_get') &&
        !window.__documentProjectHeld
      ) {
        const result = await response.clone().json();
        if (result.document?.id === documentId) {
          await new Promise((resolve) => {
            window.__documentProjectHeld = () => {
              window.fetch = original;
              resolve();
            };
          });
        }
      }
      return response;
    };
  }, summary.id);
  await main.locator('.doc-sidebar button').filter({ hasText: '알파 문서' }).click();
  await until(
    () => main.evaluate(() => Boolean(window.__documentProjectHeld)),
    'actual saved document read held',
  );
  await setProject('beta');
  await main
    .locator('.document-workspace .doc-project')
    .filter({ hasText: '프로젝트 베타' })
    .waitFor();
  await main.evaluate(() => window.__documentProjectHeld());
  await edit.waitFor();
  if ((await options.getAttribute('open')) === null) await options.locator('summary').click();
  assert.equal(await scopePicker.locator('input:checked').count(), 1);
  assert((await scopePicker.innerText()).includes('프로젝트 베타'));
  assert(!(await scopePicker.innerText()).includes('프로젝트 알파'));
  assert.equal(await edit.inputValue(), draft);
  if (await main.locator('.toast button').count()) await main.locator('.toast button').click();
  await main.setViewportSize({ width: 390, height: 760 });
  assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await main.screenshot({
    path: join(root, 'document-project-narrow.png'),
    animations: 'disabled',
  });
  for (const source of sources) assert.equal(await hash(source.file), source.sha256);
  assert.equal(await hash(original), executableSha256);
  assert.equal((await invoke('model_status')).modelReady, false);
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(data, { executable, portable: true });
  const reopened = await instance.invoke('doc_get', { id: summary.id, revisionId: null });
  assert.equal(reopened.revision.blocks[0].markdown, draft);
  assert.deepEqual(reopened.revision.scopeIds, ['alpha']);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      executableSha256,
      release: profile === '1',
      generationRequests: requests.length,
      externalAIRequests: 0,
      projectSource: 'alpha',
      crossProjectGenerationBlocked: true,
      dirtyEditsPreserved: true,
      restarted: true,
      errors: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => provider.close(done));
}
