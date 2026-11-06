// Actual filename watcher/search IPC, with one real response held for inspection.
// Browser plugin not available: own bundled WebView and isolated source/profile.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-indexing-ui-'));
const folder = join(directory, 'files');
await mkdir(folder);
await writeFile(join(folder, 'needle-first.md'), 'public filename fixture');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F7',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      { id: 'files', name: 'Files', path: folder, filenameEnabled: true, contentEnabled: false },
    ],
  }),
);
let instance;
try {
  instance = await launch(
    directory,
    process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
  );
  const { main, invoke } = instance;
  await until(
    async () => (await invoke('index_status')).initialScanComplete,
    'initial filename sweep complete',
  );
  await main.getByRole('button', { name: '파일명', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('needle');
  await main.getByRole('option').filter({ hasText: 'needle-first.md' }).waitFor();
  const action = main.getByRole('button', { name: '검색 결과로 대화', exact: true });
  assert(await action.isEnabled());
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__indexAction = document.querySelector('.search-chat-action');
    window.__heldIndexResponse = false;
    window.__restoreIndexFetch = () => {
      window.fetch = original;
      window.__releaseIndexResponse?.();
    };
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      const response = await original(input, options);
      if (url.hostname === 'ipc.localhost' && url.pathname === '/search_files') {
        window.__heldIndexResponse = true;
        await new Promise((resolve) => {
          window.__releaseIndexResponse = resolve;
        });
      }
      return response;
    };
  });
  await writeFile(join(folder, 'needle-second.md'), 'watcher really changes the index revision');
  await until(
    () => main.evaluate(() => window.__heldIndexResponse),
    'real revision-driven filename response held',
  );
  assert.equal(
    (await invoke('index_status')).initialScanComplete,
    true,
    'live changes must preserve the initial-sweep milestone',
  );
  assert.equal(
    await main.locator('.search-chat-action').count(),
    1,
    'index refresh must not unmount the action row',
  );
  assert(await action.isVisible());
  assert.equal(await action.isEnabled(), false, 'stale selections still cannot create chat');
  assert(
    await main.evaluate(
      () => window.__indexAction === document.querySelector('.search-chat-action'),
    ),
    'same DOM row survives indexing',
  );
  await main.screenshot({ path: join(directory, 'refresh-pending.png') });
  await main.evaluate(() => window.__restoreIndexFetch());
  await main.getByRole('option').filter({ hasText: 'needle-second.md' }).waitFor();
  await until(() => action.isEnabled(), 'fresh selection enabled after real response');
  await main.screenshot({ path: join(directory, 'refresh-complete.png') });
  assert.equal((await invoke('index_status')).count, 3);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({ passed: true, directory, revision: (await invoke('index_status')).revision }),
  );
} finally {
  if (instance) {
    await instance.main.evaluate(() => window.__restoreIndexFetch?.()).catch(() => {});
    await shutdown(instance);
  }
}
