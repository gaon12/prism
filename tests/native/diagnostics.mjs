// Browser plugin not available: actual bundled WebView2 + repository Playwright.
// Only marked synthetic files, an isolated profile and MemoryClipboard are used.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-diagnostics-'));
const source = join(directory, 'PRIVATE_FOLDER_SENTINEL');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(source);
await mkdir(evidence, { recursive: true });
await writeFile(join(source, 'PRIVATE_FILENAME_SENTINEL.md'), 'PRIVATE_DOCUMENT_SENTINEL');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F10',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    generation: {
      mode: 'local',
      local: {
        endpoint: 'http://127.0.0.1:39999/PRIVATE_ENDPOINT_SENTINEL',
        model: 'PRIVATE_MODEL_SENTINEL',
        maxTokens: 32,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
    scopes: [
      {
        id: 'PRIVATE_SCOPE_SENTINEL',
        name: 'PRIVATE_SCOPE_NAME_SENTINEL',
        path: source,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);
await writeFile(
  join(directory, 'session.json'),
  JSON.stringify({
    query: 'PRIVATE_SEARCH_SENTINEL',
    recentQueries: ['PRIVATE_HISTORY_SENTINEL'],
    mode: 'filename',
    extension: '',
    scopeId: null,
    selectedPath: null,
  }),
);
let instance;
try {
  instance = await launch(directory);
  const { main, invoke } = instance;
  const external = [];
  const warnings = [];
  main.on('console', (message) => {
    if (message.type() === 'warning') warnings.push(message.text());
  });
  main.on('request', (request) => {
    if (
      /^https?:/.test(request.url()) &&
      !['tauri.localhost', 'ipc.localhost'].includes(new URL(request.url()).hostname)
    )
      external.push(request.url());
  });
  // Filename catalog includes the configured root directory and its one file.
  await until(async () => (await invoke('index_status')).count === 2, 'synthetic filename indexed');
  await invoke('copy_text', { text: 'PRIVATE_CLIPBOARD_SENTINEL' });
  const before = await invoke('get_settings');
  const capture = await invoke('diagnostics_snapshot');
  assert.equal(capture.app.distribution, 'isolated');
  assert.equal(capture.filenameIndex.count, 2);
  assert.equal(capture.settings.filenameScopeCount, 1);
  assert.equal(capture.settings.contentScopeCount, 0);
  assert.equal(capture.health.documentsInitialized, true);
  assert.equal(capture.clipboard.enabled, false);
  assert.equal(capture.embedding.phase, 'notInstalled');
  assert(!JSON.stringify(capture).includes('PRIVATE_'));
  assert(!JSON.stringify(capture).includes(directory));
  assert.equal((await invoke('paste_fixture')).clipboard.text, 'PRIVATE_CLIPBOARD_SENTINEL');

  // Only intercept the file picker and external browser handoff. Collection,
  // file writing and copying continue through real native commands.
  await main.evaluate(() => {
    // Tauri's invoke property is immutable. Intercept only the fetch transport
    // for these three commands, leaving all other native IPC intact.
    const original = window.fetch.bind(window);
    window.__diagnosticTest = { savePath: null, issueOpens: 0, failure: null, saveCalls: 0 };
    window.fetch = (input, options) => {
      const test = window.__diagnosticTest;
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1));
      const response = (value, failed = false) =>
        Promise.resolve(
          new Response(JSON.stringify(value), {
            headers: {
              'Content-Type': 'application/json',
              'Tauri-Response': failed ? 'error' : 'ok',
            },
          }),
        );
      if (test.failure === command) return response('PRIVATE_RAW_ERROR_SENTINEL', true);
      if (command === 'plugin:dialog|save') {
        test.saveCalls += 1;
        return response(test.savePath);
      }
      if (command === 'diagnostics_open_issue') {
        test.issueOpens += 1;
        return response(null);
      }
      return original(input, options);
    };
  });
  await main.getByRole('button', { name: '설정', exact: true }).click();
  const dialog = main.getByRole('dialog', { name: '설정', exact: true });
  const panel = dialog.getByRole('region', { name: '오류 보고', exact: true });
  assert.equal(await panel.getByLabel('보고서 미리보기', { exact: true }).count(), 0);
  await panel.getByRole('button', { name: '진단 미리보기', exact: true }).click();
  const preview = panel.getByLabel('보고서 미리보기', { exact: true });
  await preview.waitFor();
  const note =
    '합성 재현: 파일 검색을 누른 뒤 결과가 나타나지 않았습니다.\n1. 검색 창 열기\n2. 검색어 입력';
  await panel.getByLabel('문제가 생긴 동작과 재현 순서').fill(note);
  const report = await preview.inputValue();
  assert(report.includes(note));
  assert(!report.includes('PRIVATE_'));
  assert.equal(await preview.getAttribute('readonly'), '');
  await panel.getByRole('button', { name: '보고서 복사', exact: true }).click();
  await panel.getByText('보고서를 복사했습니다.', { exact: true }).waitFor();
  assert.equal((await invoke('paste_fixture')).clipboard.text, report);

  // Cancel is not success and does not change the captured report.
  await panel.getByRole('button', { name: '파일로 저장', exact: true }).click();
  await until(
    async () => !((await panel.getAttribute('aria-busy')) === 'true'),
    'cancelled picker',
  );
  assert.equal(await main.evaluate(() => window.__diagnosticTest.saveCalls), 1);
  assert.equal(await panel.getByText('보고서를 저장했습니다.', { exact: true }).count(), 0);
  assert.equal(await preview.inputValue(), report);
  const destination = join(directory, '사용자가 선택한 보고서.md');
  await main.evaluate((path) => {
    window.__diagnosticTest.savePath = path;
  }, destination);
  await panel.getByRole('button', { name: '파일로 저장', exact: true }).click();
  await panel.getByText('보고서를 저장했습니다.', { exact: true }).waitFor();
  assert.equal(await readFile(destination, 'utf8'), report);

  // Raw native error text must not be added to either the UI or report.
  await main.evaluate(() => {
    window.__diagnosticTest.failure = 'diagnostics_snapshot';
  });
  await panel.getByRole('button', { name: '진단 새로 수집', exact: true }).click();
  await panel.getByRole('alert').waitFor();
  assert(!(await panel.innerText()).includes('PRIVATE_RAW_ERROR_SENTINEL'));
  assert.equal(await preview.inputValue(), report);
  await main.evaluate(() => {
    window.__diagnosticTest.failure = null;
  });
  await panel.getByRole('button', { name: '진단 새로 수집', exact: true }).click();
  await until(async () => (await preview.inputValue()) !== report, 'fresh diagnostic timestamp');
  assert.equal(await panel.getByLabel('문제가 생긴 동작과 재현 순서').inputValue(), note);
  await panel.getByRole('button', { name: 'GitHub에서 문제 보고', exact: true }).click();
  assert.equal(await main.evaluate(() => window.__diagnosticTest.issueOpens), 1);
  assert.equal((await invoke('get_settings')).revision, before.revision);
  assert.equal(
    await readFile(join(source, 'PRIVATE_FILENAME_SENTINEL.md'), 'utf8'),
    'PRIVATE_DOCUMENT_SENTINEL',
  );

  await preview.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(evidence, 'diagnostics-native.png'), animations: 'disabled' });
  await main.setViewportSize({ width: 390, height: 844 });
  await preview.scrollIntoViewIfNeeded();
  assert.equal(await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1), true);
  await main.screenshot({
    path: join(evidence, 'diagnostics-native-mobile.png'),
    animations: 'disabled',
  });
  await panel.getByRole('button', { name: '보고서 복사', exact: true }).click();
  await panel.getByText('보고서를 복사했습니다.', { exact: true }).waitFor();
  assert.equal((await invoke('paste_fixture')).clipboard.text, await preview.inputValue());
  await panel
    .getByRole('button', { name: 'GitHub에서 문제 보고', exact: true })
    .scrollIntoViewIfNeeded();
  await main.screenshot({
    path: join(evidence, 'diagnostics-native-mobile-actions.png'),
    animations: 'disabled',
  });
  assert.deepEqual(external, []);
  assert.deepEqual(instance.errors, []);
  assert.deepEqual(warnings, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      privateSentinelsExcluded: true,
      copyMatchesPreview: true,
      savedMatchesPreview: true,
      settingsUnchanged: true,
      externalRequests: 0,
      physicalClipboard: false,
      pickerAndIssueHandoff: 'synthetic',
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
