// Browser plugin not available. Exercise the real bundled WebView and verified
// local archive, using IPC only to start work in a second client. No API mocks.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert(process.env.PRISM_OFFICE_QA_ARCHIVE, 'Supply the qualified local ZIP.');
const archive = resolve(process.env.PRISM_OFFICE_QA_ARCHIVE);
assert.equal(
  createHash('sha256')
    .update(await readFile(archive))
    .digest('hex'),
  '79dac4566699d5df01bd0ec3b8c830f1832aebb2ef739ebe917df1d60c0d09ab',
);
const directory = await mkdtemp(join(tmpdir(), 'prism-office-panel-'));
const profile = join(directory, 'profile');
await mkdir(profile);
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F7',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [],
  }),
);
const result = { directory, screenshots: [], phase: 'launch' };
let instance;
try {
  instance = await launch(profile, { executable: process.env.PRISM_TEST_EXECUTABLE });
  const { main, invoke, errors } = instance;
  const warnings = [];
  for (const page of [main, instance.quick])
    page.on('console', (message) => {
      if (message.type() === 'warning') warnings.push(message.text());
    });
  assert.equal(await main.title(), 'Prism');
  assert.match(main.url(), /tauri\.localhost|tauri:/);
  await expect(main.locator('vite-error-overlay')).toHaveCount(0);
  const dialog = () => main.getByRole('dialog', { name: '설정', exact: true });
  const panel = () => dialog().getByRole('region', { name: 'Office 원본 페이지', exact: true });
  const openSettings = async () => {
    await main.getByRole('button', { name: '설정', exact: true }).first().click();
    await expect(panel()).toBeVisible();
    await panel().scrollIntoViewIfNeeded();
  };
  const closeSettings = () => dialog().getByRole('button', { name: '닫기', exact: true }).click();
  const screenshot = async (name) => {
    await panel().scrollIntoViewIfNeeded();
    const path = join(directory, name);
    await main.screenshot({ path });
    result.screenshots.push(path);
  };
  const startInstall = async (path) =>
    main.evaluate((path) => {
      window.__officePanelInstall = window.__TAURI_INTERNALS__
        .invoke('collection_office_install', { path })
        .then(
          () => ({ ok: true }),
          (error) => ({ error }),
        );
    }, path);
  await openSettings();
  await expect(panel().getByText('엔진 없음', { exact: true })).toBeVisible();
  await expect(panel().getByRole('button', { name: '엔진 ZIP 선택' })).toBeEnabled();
  await expect(panel().getByText(/다운로드용 패키지는 아직 공개되지/)).toBeVisible();
  await expect(panel().getByRole('progressbar')).toHaveCount(0);
  await screenshot('engine-absent.png');

  result.phase = 'retained-failure';
  await startInstall(join(directory, 'missing.zip'));
  assert((await main.evaluate(() => window.__officePanelInstall)).error);
  await expect(panel().getByRole('alert')).toBeVisible();
  const failure = await invoke('collection_office_status');
  assert(failure.lastError && !failure.installing && !failure.installed);
  await closeSettings();
  await openSettings();
  await expect(panel().getByRole('alert')).toBeVisible();

  result.phase = 'cancel-and-remount';
  await startInstall(archive);
  await expect(panel().getByRole('progressbar')).toBeVisible();
  await closeSettings();
  await openSettings();
  await expect(panel().getByRole('progressbar')).toBeVisible();
  await panel().getByRole('button', { name: '설치 중단', exact: true }).click();
  assert.equal((await main.evaluate(() => window.__officePanelInstall)).error.code, 'cancelled');
  await expect(panel().getByText('설치를 중단했습니다.', { exact: true })).toBeVisible();
  await expect(panel().getByRole('button', { name: '엔진 ZIP 선택' })).toBeEnabled();
  assert.deepEqual(
    await readdir(join(profile, 'office')).catch((error) => {
      if (error.code === 'ENOENT') return [];
      throw error;
    }),
    [],
  );
  await closeSettings();
  await openSettings();
  await expect(panel().getByText('설치를 중단했습니다.', { exact: true })).toBeVisible();
  await screenshot('engine-cancelled.png');

  result.phase = 'install-and-progress';
  await main.evaluate(() => {
    window.__officePanelNode = document.querySelector('.office-engine-panel');
    window.__officeSettingsNode = document.querySelector('.settings-modal');
  });
  await startInstall(archive);
  await expect(panel().getByRole('progressbar')).toBeVisible();
  await main.evaluate(() => {
    window.__officeProgressNode = document.querySelector('.office-engine-panel progress');
  });
  await until(
    async () => (await invoke('collection_office_status')).progress?.files > 0,
    'real archive file progress',
    180_000,
  );
  assert(
    await main.evaluate(
      () =>
        window.__officePanelNode === document.querySelector('.office-engine-panel') &&
        window.__officeSettingsNode === document.querySelector('.settings-modal'),
    ),
  );
  const boxes = await panel().evaluate((node) => {
    const capacity = node.querySelector('.office-engine-capacity').getBoundingClientRect();
    const actions = node.querySelector('.office-engine-actions').getBoundingClientRect();
    return { capacityBottom: capacity.bottom, actionsTop: actions.top };
  });
  assert(boxes.actionsTop > boxes.capacityBottom + 15, 'actions are separated from size text');
  await screenshot('engine-installing.png');
  await expect(panel().getByRole('alert')).toHaveCount(0);
  await until(
    async () => (await invoke('collection_office_status')).progress?.phase === 'finalizing',
    'real package reaches final verification and promotion',
    240_000,
  );
  await expect(panel().getByText('설치를 마무리하는 중', { exact: true })).toBeVisible();
  assert.equal(await panel().getByRole('progressbar').getAttribute('value'), null);
  assert(
    await main.evaluate(
      () =>
        window.__officeProgressNode === document.querySelector('.office-engine-panel progress') &&
        window.__officePanelNode === document.querySelector('.office-engine-panel') &&
        window.__officeSettingsNode === document.querySelector('.settings-modal'),
    ),
  );
  result.finalizingRetainedDom = true;
  result.finalizingIndeterminate = true;
  await screenshot('engine-finalizing.png');
  await closeSettings();
  // Complete with settings unmounted, then recover its terminal status.
  await until(
    async () => !(await invoke('collection_office_status')).installing,
    'verified installation completes',
    240_000,
  );
  assert.deepEqual(await main.evaluate(() => window.__officePanelInstall), { ok: true });
  await openSettings();
  await expect(panel().getByText('엔진 파일 있음', { exact: true })).toBeVisible();
  await expect(panel().getByText('엔진 설치를 마쳤습니다.', { exact: true })).toBeVisible();
  await expect(panel().getByRole('progressbar')).toHaveCount(0);
  await expect(panel().getByRole('button', { name: '엔진 ZIP 선택' })).toBeEnabled();
  await screenshot('engine-installed.png');
  await main.setViewportSize({ width: 1024, height: 768 });
  await panel().scrollIntoViewIfNeeded();
  assert(await dialog().evaluate((node) => node.scrollWidth <= node.clientWidth + 1));
  await screenshot('engine-installed-1024.png');
  const status = await invoke('collection_office_status');
  assert(status.installed && !status.installing && !status.lastError);
  assert(status.revision > failure.revision);
  assert.equal(status.downloadAvailable, false);
  assert.deepEqual(errors, []);
  assert.deepEqual(warnings, []);
  result.status = status;
  result.consoleErrors = errors;
  result.consoleWarnings = warnings;
  result.sameProgressNodes = true;
  result.remountFailureCancelSuccess = true;
  result.passed = true;
  result.phase = 'passed';
  console.log(JSON.stringify(result));
} catch (error) {
  result.error = error instanceof Error ? error.message : error;
  if (instance)
    result.failureStatus = await instance.invoke('collection_office_status').catch(() => null);
  if (instance)
    await instance.main.screenshot({ path: join(directory, 'failure.png') }).catch(() => {});
  throw error;
} finally {
  if (instance) await shutdown(instance);
  await writeFile(join(directory, 'result.json'), JSON.stringify(result, null, 2));
}
