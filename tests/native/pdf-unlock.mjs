// Bundled WebView -> actual encrypted PDF parser -> SQLite and real Q8/Lance.
// Browser plugin not available: use the repository's Playwright/WebView2 harness.
// Every source and profile belongs to this test. Never use the system clipboard.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-pdf-unlock-'));
const folder = join(directory, 'source-pdfs');
const privateFolder = join(directory, 'filename-only');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all([folder, privateFolder, evidence].map((p) => mkdir(p, { recursive: true })));
const aes = join(folder, '암호 자료.pdf');
const rc4 = join(folder, '소유자 암호.pdf');
const cancelled = join(folder, '취소 자료.pdf');
const outside = join(privateFolder, '비공개.pdf');
await Promise.all([
  copyFile('tests/fixtures/encrypted-pdf/aes256.pdf', aes),
  copyFile('tests/fixtures/encrypted-pdf/rc4-128.pdf', rc4),
  copyFile('tests/fixtures/encrypted-pdf/aes256.pdf', cancelled),
  copyFile('tests/fixtures/encrypted-pdf/aes256.pdf', outside),
]);
const passwords = ['암호-fixture-密碼', 'fixture-owner-password', 'wrong-unlock-canary'];
const original = await readFile(aes);
const sha = createHash('sha256').update(original).digest('hex');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      { id: 'pdf', name: 'PDF', path: folder, filenameEnabled: true, contentEnabled: true },
      {
        id: 'private',
        name: '파일명만',
        path: privateFolder,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
const options = process.env.PRISM_TEST_EXECUTABLE
  ? { executable: process.env.PRISM_TEST_EXECUTABLE }
  : {};
let instance;
// Keep the actual structured IPC error intact across Playwright's evaluator.
// Letting it reject out of evaluate would wrap it as an unstructured JS Error.
const rejectedUnlock = (args) =>
  instance.main.evaluate(async (args) => {
    try {
      await window.__TAURI_INTERNALS__.invoke('content_unlock_pdf', args);
      return { unexpectedSuccess: true };
    } catch (error) {
      return error;
    }
  }, args);
try {
  instance = await launch(directory, options);
  let { main, invoke } = instance;
  await until(async () => {
    const s = await invoke('content_status');
    return s.phase === 'ready' && s.issues.filter((i) => i.code === 'encrypted').length === 3;
  }, 'three encrypted source issues');
  console.log(JSON.stringify({ directory, phase: 'encrypted-sources-listed' }));
  await invoke('content_cancel_index');
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.locator('.content-issues summary').click();
  const opener = main.getByRole('button', { name: '암호 입력: 암호 자료.pdf', exact: true });
  await opener.click();
  let dialog = main.getByRole('dialog', { name: '암호 PDF 다시 색인' });
  assert((await dialog.innerText()).includes('로컬 검색 색인에 저장'));
  await dialog.getByLabel('PDF 비밀번호', { exact: true }).fill(passwords[2]);
  await dialog.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await dialog.getByRole('alert').filter({ hasText: '암호가 맞지 않거나' }).waitFor();
  assert.equal(await dialog.getByLabel('PDF 비밀번호', { exact: true }).inputValue(), '');
  assert.equal((await invoke('content_status')).indexed, 0);
  await main.screenshot({ path: join(evidence, 'pdf-unlock-rejected.png') });
  await main.setViewportSize({ width: 390, height: 760 });
  await main.screenshot({ path: join(evidence, 'pdf-unlock-mobile.png') });
  assert(await dialog.evaluate((element) => element.getBoundingClientRect().right <= innerWidth));
  await main.setViewportSize({ width: 1600, height: 1050 });
  await dialog.getByLabel('PDF 비밀번호', { exact: true }).fill(passwords[0]);
  await dialog.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  assert.equal((await invoke('content_status')).phase, 'cancelled', 'manual retry preserves pause');
  assert.equal((await invoke('model_status')).phase, 'notInstalled', 'unlock never starts a model');
  assert.deepEqual(await readFile(aes), original, 'encrypted source is byte-identical');
  const pdfFiles = (await readdir(directory, { recursive: true, withFileTypes: true }))
    .filter((entry) => entry.isFile() && /\.pdf$/iu.test(entry.name))
    .map((entry) => join(entry.parentPath, entry.name));
  assert.deepEqual(
    pdfFiles.sort(),
    [aes, rc4, cancelled, outside].sort(),
    'no decrypted PDF copy is written',
  );
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('unlockcanarythird');
  await main.getByRole('option').filter({ hasText: '암호 자료.pdf' }).waitFor();
  await main.screenshot({ path: join(evidence, 'pdf-unlock-result.png') });
  const search = () =>
    invoke('search_content', {
      query: 'unlockcanarythird',
      extension: '',
      scopeId: null,
      requestId: crypto.randomUUID(),
    });
  const indexed = await search();
  assert.equal(indexed.hits.length, 1);
  assert.equal(indexed.hits[0].chunk.page, 3);
  assert.equal(indexed.hits[0].chunk.fingerprint.sha256, sha);
  // An early cancellation, replayed token, and stale token must never parse.
  let token = await invoke('content_prepare_unlock');
  await invoke('content_cancel_unlock', { requestId: token });
  assert.equal(
    (await rejectedUnlock({ path: cancelled, password: passwords[0], requestId: token })).code,
    'cancelled',
  );
  const stale = await invoke('content_prepare_unlock');
  token = await invoke('content_prepare_unlock');
  await invoke('content_cancel_unlock', { requestId: stale });
  assert.equal(
    (await rejectedUnlock({ path: cancelled, password: passwords[0], requestId: stale })).code,
    'cancelled',
  );
  await invoke('content_cancel_unlock', { requestId: token });
  token = await invoke('content_prepare_unlock');
  assert.equal(
    (await rejectedUnlock({ path: outside, password: passwords[0], requestId: token })).code,
    'outsideScope',
  );
  // Close the dialog while prepare is in flight. Delay only the real response;
  // parser, token registry and transport are not replaced.
  await main.getByRole('button', { name: '암호 입력: 취소 자료.pdf', exact: true }).click();
  dialog = main.getByRole('dialog', { name: '암호 PDF 다시 색인' });
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__pdfPrepareSeen = false;
    window.__pdfUnlockCalls = 0;
    window.__pdfCancelCalls = 0;
    window.__restorePdfInvoke = () => {
      window.fetch = original;
    };
    // Tauri's invoke property is immutable. Delay its actual HTTP IPC response
    // instead; no Rust command, parser, password result or token is substituted.
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      const command = url.hostname === 'ipc.localhost' ? url.pathname : '';
      if (command === '/content_unlock_pdf') window.__pdfUnlockCalls++;
      const response = await original(input, options);
      if (command === '/content_cancel_unlock') window.__pdfCancelCalls++;
      if (command === '/content_prepare_unlock') {
        window.__pdfPrepareSeen = true;
        await new Promise((done) => {
          window.__releasePdfPrepare = done;
        });
      }
      return response;
    };
  });
  await dialog.getByLabel('PDF 비밀번호', { exact: true }).fill(passwords[0]);
  await dialog.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await until(() => main.evaluate(() => window.__pdfPrepareSeen), 'real prepare response delayed');
  await dialog.getByRole('button', { name: '취소', exact: true }).click();
  await main.evaluate(() => window.__releasePdfPrepare());
  await until(
    () =>
      main.evaluate(
        () =>
          window.__pdfCancelCalls > 0 &&
          window.__pdfUnlockCalls === 0 &&
          !document.querySelector('dialog.pdf-unlock-dialog'),
      ),
    'closed dialog never sends a password',
  );
  await main.evaluate(() => window.__restorePdfInvoke());
  assert.equal((await search()).hits.length, 1, 'cancelled PDF never publishes');
  await main.getByRole('button', { name: '암호 입력: 소유자 암호.pdf', exact: true }).click();
  dialog = main.getByRole('dialog', { name: '암호 PDF 다시 색인' });
  await dialog.getByLabel('PDF 비밀번호', { exact: true }).fill(passwords[1]);
  await dialog.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  assert.equal((await search()).hits.length, 2, 'legacy owner password produces searchable text');
  console.log(JSON.stringify({ directory, phase: 'ui-cancellation-and-passwords-verified' }));
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory, options);
  ({ main, invoke } = instance);
  await until(
    async () => (await invoke('content_status')).phase === 'ready',
    'cache reconciliation after restart',
  );
  assert.equal(
    (await search()).hits.length,
    2,
    'unlocked text survives restart without the password',
  );
  const persisted = await Promise.all(
    ['settings.json', 'session.json', 'content/content.sqlite3', 'content/content.sqlite3-wal'].map(
      async (p) => {
        try {
          return await readFile(join(directory, p));
        } catch (error) {
          if (error.code === 'ENOENT') return Buffer.alloc(0);
          throw error;
        }
      },
    ),
  );
  for (const secret of passwords)
    for (const bytes of persisted)
      assert(
        !bytes.includes(Buffer.from(secret)),
        'password is absent from persisted settings/index',
      );
  console.log(JSON.stringify({ directory, phase: 'password-free-reopen-verified' }));
  // The compact Spotlight uses the same dialog. Escape closes the password
  // entry rather than dismissing the whole quick-search window.
  await invoke('show_quick');
  const quick = instance.quick;
  await quick.getByRole('textbox', { name: '검색어', exact: true }).fill('unlockcanarythird');
  await quick.locator('.content-issues summary').click();
  await quick.getByRole('button', { name: '암호 입력: 취소 자료.pdf', exact: true }).click();
  const quickDialog = quick.getByRole('dialog', { name: '암호 PDF 다시 색인' });
  await quickDialog.waitFor();
  await quick.screenshot({ path: join(evidence, 'pdf-unlock-quick.png') });
  await quickDialog.getByLabel('PDF 비밀번호', { exact: true }).fill(passwords[0]);
  await quickDialog.getByLabel('PDF 비밀번호', { exact: true }).press('Escape');
  await quickDialog.waitFor({ state: 'hidden' });
  assert.equal(
    await quick.evaluate(() =>
      window.__TAURI_INTERNALS__.invoke('plugin:window|is_visible', { label: 'quick' }),
    ),
    true,
  );
  assert.equal((await search()).hits.length, 2);
  await invoke('hide_quick');
  // The production runtime embeds cached unlocked text, never a fake vector.
  await invoke('model_import', {
    path: resolve('.dev/models/embeddinggemma-2-Q8_0.gguf'),
    runtime: false,
  });
  await invoke('model_import', {
    path: resolve('.dev/runtime/llama-b11960-bin-win-cpu-x64.zip'),
    runtime: true,
  });
  console.log(JSON.stringify({ directory, phase: 'pinned-model-and-runtime-imported' }));
  await invoke('model_start');
  await until(
    async () => (await invoke('model_status')).phase === 'running',
    'pinned Q8 runtime',
    120000,
  );
  await invoke('content_refresh');
  await until(
    async () => {
      const status = await invoke('content_status');
      return status.phase === 'ready' && status.embedded === 2 && !status.embeddingError;
    },
    'unlocked PDFs embedded in actual LanceDB',
    120000,
  );
  // Search cancellation is window-scoped. Leave the UI query idle before a
  // direct semantic request so its revision-driven keyword refresh cannot
  // supersede the request being verified here.
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('');
  await until(async () => (await invoke('get_search_session')).query === '', 'idle search session');
  const outcome = await main.evaluate(async () => {
    try {
      return {
        ok: true,
        value: await window.__TAURI_INTERNALS__.invoke('search_semantic', {
          query: 'evidence page three',
          extension: '',
          scopeId: null,
          requestId: crypto.randomUUID(),
        }),
      };
    } catch (error) {
      return { ok: false, error };
    }
  });
  assert(outcome.ok, JSON.stringify(outcome.error));
  const semantic = outcome.value;
  assert(semantic.hits.some((hit) => hit.chunk.fingerprint.sha256 === sha && hit.chunk.page === 3));
  assert(
    semantic.hits.every(
      (hit) => hit.chunk.filePath !== cancelled && hit.chunk.filePath !== outside,
    ),
  );
  assert.deepEqual(await readFile(aes), original);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      executable: options.executable ?? 'default',
      model: 'actual pinned EmbeddingGemma 2 Q8',
      sha,
      keywordFiles: 2,
      semanticHits: semantic.hits.length,
    }),
  );
} catch (error) {
  if (instance)
    await instance.main
      .screenshot({ path: join(evidence, 'pdf-unlock-failure.png') })
      .catch(() => {});
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
