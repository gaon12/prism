// Real SCM/preflight, real IPC and actual WebView UI. No service/model response
// replacement, UAC prompt, registry write, installation or real clipboard access.
// Browser plugin not available; use the test-owned isolated native app.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-service-ui-'));
const folder = join(directory, 'files');
await mkdir(folder);
await writeFile(join(folder, 'initial.md'), 'public fixture');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    theme: 'light',
    language: 'ko',
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
  const { main, invoke, errors } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.platform, 'windows');
  assert.equal(
    runtime.installKind,
    'isolated',
    'never run mutation regression in an ordinary profile',
  );
  const before = await invoke('index_service_status');
  assert.equal(before.managementAllowed, false);
  assert.equal(
    before.canInstall || before.canStart || before.canUpgrade || before.canRemove,
    false,
  );
  const helperFolders = async () =>
    (await readdir(tmpdir())).filter((name) => name.startsWith('Prism-index-helper-')).sort();
  const stagedBefore = await helperFolders();
  for (const action of ['install', 'start', 'upgrade', 'uninstall']) {
    // Playwright wraps rejected plain objects as "page.evaluate: Object".
    // Preserve the actual native error as data, without replacing the IPC.
    const result = await main.evaluate(async (action) => {
      try {
        await window.__TAURI_INTERNALS__.invoke('index_service_manage', { action });
        return { rejected: false, code: null };
      } catch (error) {
        return { rejected: true, code: error.code };
      }
    }, action);
    assert.deepEqual(result, { rejected: true, code: 'isolated' });
  }
  assert.deepEqual(
    await helperFolders(),
    stagedBefore,
    'isolated commands must reject before helper extraction/UAC',
  );
  assert.equal((await invoke('index_service_status')).state, before.state);
  await invoke('open_settings', { tab: 'scopes' });
  const dialog = main.getByRole('dialog', { name: '설정', exact: true });
  const panel = dialog.getByRole('region', { name: '빠른 파일명 색인', exact: true });
  await expect(panel).toContainText('격리된 테스트에서는 서비스 상태만');
  if (['running', 'stopped'].includes(before.state)) {
    await expect(
      panel.getByRole('button', { name: '앱 내장 버전 적용', exact: true }),
    ).toBeDisabled();
    await expect(panel).toContainText('다른 Prism 사본의 파일 수집도 잠시 중단될 수 있습니다.');
  }
  for (const button of await panel.locator('.index-service-actions > button').all()) {
    await expect(button).toBeDisabled();
  }
  const refresh = panel.getByRole('button', { name: '서비스 상태 새로 확인', exact: true });
  await expect(refresh).toBeEnabled();
  await main.evaluate(() => {
    window.__serviceDialogNode = document.querySelector('.settings-modal');
    window.__servicePanelNode = document.querySelector('.index-service-panel');
  });
  for (let n = 0; n < 5; n++) {
    await refresh.click();
    await expect(refresh).toBeEnabled();
    await writeFile(join(folder, `live-${n}.md`), 'public live watcher fixture');
  }
  await until(async () => (await invoke('index_status')).count >= 7, 'real filename watcher');
  const stable = await main.evaluate(() => ({
    dialog: window.__serviceDialogNode === document.querySelector('.settings-modal'),
    panel: window.__servicePanelNode === document.querySelector('.index-service-panel'),
    opacity: getComputedStyle(document.querySelector('.settings-modal')).opacity,
    overflow: document.documentElement.scrollWidth > innerWidth,
  }));
  assert.deepEqual(stable, { dialog: true, panel: true, opacity: '1', overflow: false });
  await panel.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(directory, 'service-ko-light.png') });
  assert(
    !/S-1-5-|C:\\/.test(await panel.innerText()),
    'system paths and ACL identities stay out of the service panel',
  );
  await dialog.getByRole('button', { name: '닫기', exact: true }).click();
  const settings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...settings, theme: 'dark', language: 'en' },
    expectedRevision: settings.revision,
  });
  await invoke('open_settings', { tab: 'scopes' });
  const english = main.getByRole('dialog', { name: 'Settings', exact: true });
  const translated = english.getByRole('region', { name: 'Fast filename indexing', exact: true });
  await expect(translated).toContainText('Isolated tests can only view service status.');
  if (['running', 'stopped'].includes(before.state)) {
    await expect(
      translated.getByRole('button', { name: 'Use bundled version', exact: true }),
    ).toBeDisabled();
  }
  await main.setViewportSize({ width: 760, height: 700 });
  await translated.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(directory, 'service-en-dark-760.png') });
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  assert.equal((await invoke('index_service_status')).state, before.state);
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      directory,
      state: before.state,
      issue: before.issue,
      stable,
      errors: errors.length,
    }),
  );
} finally {
  if (instance) await shutdown(instance, directory);
}
