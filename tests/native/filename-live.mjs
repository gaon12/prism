// Explicit whole-C measurement in a new portable copy. Uses the genuine supplied
// binary and existing service, with no installation, elevation, model or content
// indexing. Browser plugin not available: own bundled WebView/Playwright harness.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32', 'This check requires Windows.');
assert.equal(
  process.env.PRISM_TEST_FILENAME_LIVE_SERVICE,
  'C',
  'Explicit whole-C opt-in required.',
);
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the genuine candidate executable.');
const expectRelease = process.env.PRISM_TEST_EXPECT_RELEASE !== '0';
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const executableSha256 = createHash('sha256')
  .update(await readFile(original))
  .digest('hex');
const directory = await mkdtemp(join(tmpdir(), 'prism-filename-live-'));
const portable = join(directory, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
assert.equal(
  createHash('sha256')
    .update(await readFile(executable))
    .digest('hex'),
  executableSha256,
);
await writeFile(join(portable, 'prism.portable'), '');
await mkdir(join(portable, 'notices'));
for (const name of ['pdf-renderer', 'document-pdf', 'document-hwpx', 'office-passwords']) {
  const before = await readFile(join(dirname(original), 'notices', `${name}.txt`));
  const target = join(portable, 'notices', `${name}.txt`);
  await copyFile(join(dirname(original), 'notices', `${name}.txt`), target);
  assert.deepEqual(await readFile(target), before);
}
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F7',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'drive-c',
        name: 'C',
        path: 'C:\\',
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);
let instance;
let stage = 'launch';
const start = performance.now();
try {
  // Release ignores the development data override. The real portable marker
  // and prewritten settings must be present before launch, including clipboard
  // opt-out; no ordinary installed-user profile is launched for this check.
  instance = await launch(data, { executable, portable: true });
  const mountedMs = Math.round(performance.now() - start);
  const { main, quick, invoke } = instance;
  stage = 'runtime profile and portable data';
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.native, true);
  assert.equal(runtime.development, !expectRelease, 'Validate the actual requested build profile.');
  assert.equal(runtime.installKind, 'portable');
  assert.equal(runtime.startupError, null);
  const settings = await invoke('get_settings');
  assert.equal(settings.clipboard.enabled, false);
  assert.equal(settings.startOnLogin, false);
  assert.equal(settings.updates.autoCheck, false);
  assert.equal(settings.scopes.length, 1);
  assert.equal(settings.scopes[0].filenameEnabled, true);
  assert.equal(settings.scopes[0].contentEnabled, false);
  assert.equal(settings.scopes[0].externalAllowed, false);
  const service = await invoke('index_service_status');
  assert.equal(service.state, 'running', 'Only an existing running service may be used.');
  await until(
    async () => (await invoke('index_status')).initialScanComplete,
    'complete initial C-drive filename snapshot',
    120_000,
  );
  const snapshotMs = Math.round(performance.now() - start);
  const status = await invoke('index_status');
  assert(status.count > 0);
  assert.equal(status.warnings.length, 0);
  const searchStart = performance.now();
  const result = await invoke('search_files', {
    query: 'prism',
    extension: '',
    scopeId: 'drive-c',
  });
  const searchMs = Math.round(performance.now() - searchStart);
  assert(result.total > 0 && result.results.length > 0);
  assert(result.results.every((row) => /^C:\\/iu.test(row.path)));
  stage = 'main filename mode';
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  stage = 'main filename results';
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('prism');
  await main.getByRole('option').first().waitFor();
  const renderedMs = Math.round(performance.now() - start);
  // Store only the progress summary, never live filenames or file contents.
  await main
    .locator('.index-summary')
    .first()
    .screenshot({ path: join(directory, 'index-summary.png') });
  stage = 'quick filename results';
  await invoke('show_quick');
  const quickInput = quick.getByRole('textbox', { name: '검색어', exact: true });
  await quickInput.fill('prism');
  await quick.getByRole('option').first().waitFor();
  await quickInput.press('Escape');
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  assert.equal((await invoke('content_status')).indexed, 0);
  assert.deepEqual((await invoke('get_settings')).scopes, settings.scopes);
  assert(
    (await readdir(data)).includes('WebView2'),
    'Portable WebView data stays beside the copy.',
  );
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      directory,
      original,
      executableSha256,
      release: expectRelease,
      installKind: runtime.installKind,
      mountedMs,
      snapshotMs,
      searchMs,
      renderedMs,
      indexed: status.count,
      firstSnapshotComplete: status.initialScanComplete,
      warnings: status.warnings.length,
      service: service.state,
      contentIndexed: 0,
      modelStarted: false,
      quickSearch: true,
      errors: 0,
    }),
  );
} catch (error) {
  // Controls only: a failure report must not dump the whole page's live paths.
  const controls = instance
    ? await instance.main
        .evaluate(() => ({
          ready: document.readyState,
          headings: Array.from(document.querySelectorAll('.page-heading > h1')).map(
            (node) => node.textContent,
          ),
          modes: Array.from(document.querySelectorAll('.search-modes button')).map(
            (node) => node.textContent,
          ),
        }))
        .catch(() => null)
    : null;
  console.error(JSON.stringify({ stage, executableSha256, controls }));
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
