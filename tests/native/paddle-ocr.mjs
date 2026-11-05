// Browser plugin not available: real bundled WebView and native OCR commands.
// No model responses, IPC results or progress events are replaced. The first
// install downloads pinned official artifacts and is cancelled through the UI.
// A separately materialized, verified runtime seeds the restart/inference phase.
import assert from 'node:assert/strict';
import { cp, copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert(process.env.PRISM_TEST_PADDLE_RUNTIME, 'Set PRISM_TEST_PADDLE_RUNTIME');
const prepared = resolve(process.env.PRISM_TEST_PADDLE_RUNTIME);
const catalog = JSON.parse(await readFile('tools/ocr-runtime/windows-x64.lock.json', 'utf8'));
const directory = await mkdtemp(join(tmpdir(), 'prism-paddle-ui-'));
const profile = join(directory, 'profile');
const allowed = join(directory, 'allowed');
const privateFolder = join(directory, 'filename-only');
await Promise.all([profile, allowed, privateFolder].map((path) => mkdir(path)));
await copyFile('tests/fixtures/ocr/bilingual.png', join(allowed, '한국어.png'));
await copyFile('tests/fixtures/ocr/mixed.pdf', join(allowed, 'mixed.pdf'));
await copyFile('tests/fixtures/ocr/bilingual.png', join(privateFolder, 'private.png'));
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
    ocr: { enabled: false, engine: 'auto', language: 'auto' },
    scopes: [
      {
        id: 'allowed',
        name: 'Allowed',
        path: allowed,
        filenameEnabled: true,
        contentEnabled: true,
      },
      {
        id: 'private',
        name: 'Filename only',
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
try {
  instance = await launch(profile, options);
  let { main, invoke } = instance;
  assert.equal((await invoke('runtime_info')).installKind, 'isolated');
  const settingsBefore = await invoke('get_settings');
  await invoke('open_settings', { tab: 'scopes' });
  let dialog = main.getByRole('dialog', { name: '설정', exact: true });
  let card = dialog.locator('.ocr-model-card');
  await expect(card.getByText('설치되지 않음', { exact: true })).toBeVisible();
  await dialog.getByLabel('인식 엔진', { exact: true }).selectOption('paddleVl16');
  await main.evaluate(() => {
    window.__paddleDialog = document.querySelector('dialog[open]');
    window.__paddleCard = document.querySelector('.ocr-model-card');
  });
  await card.getByRole('button', { name: '로컬 OCR 모델 설치', exact: true }).click();
  await until(
    async () => (await invoke('ocr_status')).phase === 'installing',
    'real download job starts',
  );
  await expect(card.getByRole('progressbar', { name: '다운로드', exact: true })).toBeVisible();
  await card.scrollIntoViewIfNeeded();
  const stable = await main.evaluate(async () => {
    let minOpacity = 1;
    let same = true;
    for (let i = 0; i < 24; i++) {
      await new Promise(requestAnimationFrame);
      same &&=
        document.querySelector('dialog[open]') === window.__paddleDialog &&
        document.querySelector('.ocr-model-card') === window.__paddleCard;
      minOpacity = Math.min(
        minOpacity,
        Number(getComputedStyle(window.__paddleDialog).opacity),
        Number(getComputedStyle(window.__paddleCard).opacity),
      );
    }
    return { same, minOpacity };
  });
  assert.deepEqual(stable, { same: true, minOpacity: 1 });
  await expect(dialog.getByLabel('인식 엔진', { exact: true })).toHaveValue('paddleVl16');
  await main.screenshot({ path: join(directory, 'install-ko-light.png') });
  await card.getByRole('button', { name: '모델 설치 중단', exact: true }).click();
  await until(
    async () => (await invoke('ocr_status')).phase === 'cancelled',
    'actual cancelled install',
  );
  await expect(
    card.getByText('설치가 중단되었습니다. 다시 시작할 수 있습니다.', { exact: true }),
  ).toBeVisible();
  assert.equal(
    (await readdir(join(profile, 'ocr'))).some((name) => name.startsWith('stage-')),
    false,
  );
  await expect(dialog.getByLabel('인식 엔진', { exact: true })).toHaveValue('paddleVl16');
  const unchanged = await invoke('get_settings');
  assert.deepEqual(
    unchanged.ocr,
    settingsBefore.ocr,
    'install/cancel does not save the pending OCR choice',
  );
  assert.deepEqual(unchanged.scopes, settingsBefore.scopes);
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = undefined;

  // Data setup, not an inference substitute: the app authenticates the complete
  // copied inventory and runs the actual official pipeline itself after restart.
  const installed = join(
    profile,
    'ocr',
    `paddle-v1.6-${catalog.target}-${catalog.inventorySha256.slice(0, 16)}`,
  );
  await cp(prepared, installed, { recursive: true, force: false, errorOnExist: true });
  instance = await launch(profile, options);
  ({ main, invoke } = instance);
  assert.equal((await invoke('ocr_status')).available, true);
  await invoke('open_settings', { tab: 'scopes' });
  dialog = main.getByRole('dialog', { name: '설정', exact: true });
  card = dialog.locator('.ocr-model-card');
  await expect(card.getByText('설치됨 · 인식할 때 실행', { exact: true })).toBeVisible();
  await dialog.getByLabel('인식 엔진', { exact: true }).selectOption('paddleVl16');
  await dialog
    .getByRole('checkbox', { name: '새 이미지·스캔 페이지의 글자 읽기', exact: true })
    .check();
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  const search = () =>
    invoke('search_content', {
      query: '74829',
      extension: '',
      scopeId: null,
      requestId: crypto.randomUUID(),
    });
  let hits;
  await until(
    async () => {
      const status = await invoke('content_status');
      if (status.phase !== 'ready') return false;
      hits = await search();
      return (
        hits.hits.some((hit) => hit.chunk.filePath.endsWith('한국어.png')) &&
        hits.hits.some((hit) => hit.chunk.filePath.endsWith('mixed.pdf') && hit.chunk.page === 2)
      );
    },
    'real Paddle image and selected scanned PDF page enter the scoped content index',
    240_000,
  );
  assert(!hits.hits.some((hit) => hit.chunk.filePath.endsWith('private.png')));
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  const enabled = await invoke('get_settings');
  assert.equal(enabled.ocr.engine, 'paddleVl16');
  assert.deepEqual(enabled.scopes, settingsBefore.scopes);
  // A new source proves Auto really uses the installed fallback for a missing
  // OS language; old retained text cannot satisfy this assertion.
  await invoke('save_settings', {
    settings: { ...enabled, ocr: { ...enabled.ocr, engine: 'auto', language: 'zz-ZZ' } },
    expectedRevision: enabled.revision,
  });
  await copyFile('tests/fixtures/ocr/bilingual.png', join(allowed, 'auto-fallback.png'));
  await until(
    async () => {
      if ((await invoke('content_status')).phase !== 'ready') return false;
      return (await search()).hits.some((hit) => hit.chunk.filePath.endsWith('auto-fallback.png'));
    },
    'automatic local fallback for an unavailable OS language',
    240_000,
  );
  const afterFallback = await invoke('get_settings');
  await invoke('save_settings', {
    settings: {
      ...afterFallback,
      ocr: { ...afterFallback.ocr, engine: 'paddleVl16', language: 'auto', enabled: false },
      theme: 'dark',
      language: 'en',
    },
    expectedRevision: afterFallback.revision,
  });
  await until(
    async () => (await invoke('content_status')).phase === 'ready',
    'OCR disable reconciliation',
  );
  assert(
    (await search()).hits.some((hit) => hit.chunk.filePath.endsWith('한국어.png')),
    'disabling OCR keeps indexed text',
  );
  await main.setViewportSize({ width: 760, height: 700 });
  await invoke('open_settings', { tab: 'scopes' });
  dialog = main.getByRole('dialog', { name: 'Settings', exact: true });
  card = dialog.locator('.ocr-model-card');
  await card.scrollIntoViewIfNeeded();
  await expect(dialog.getByLabel('Recognition engine', { exact: true })).toHaveValue('paddleVl16');
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await main.screenshot({ path: join(directory, 'installed-en-dark-760.png') });
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      directory,
      installerCancelledThroughUi: true,
      stable,
      seededVerifiedRuntime: true,
      actualPaddleIndexed: true,
      automaticFallbackVerified: true,
      originalScopePermissionsPreserved: true,
      retainedTextAfterDisable: true,
      errors: 0,
    }),
  );
} catch (error) {
  if (instance)
    await instance.main.screenshot({ path: join(directory, 'failure.png') }).catch(() => {});
  console.log(JSON.stringify({ directory }));
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
