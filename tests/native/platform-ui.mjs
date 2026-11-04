// Actual Windows WebView2 UI, with only platform-specific IPC responses replaced.
// This checks client behavior, not macOS/Linux execution. OS store tests live in platform-qa.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-platform-ui-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F10',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    scopes: [],
    generation: {
      local: {
        endpoint: 'http://127.0.0.1:39999/v1',
        model: 'synthetic',
        maxTokens: 32,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
  }),
);
let instance;
try {
  instance = await launch(directory);
  const { main, invoke } = instance;
  const baseline = await invoke('model_status');
  assert.equal(baseline.runtimeTarget, 'windows-x86_64');
  assert.equal(baseline.runtimeFilename, 'llama-b11960-bin-win-cpu-x64.zip');
  assert.equal(baseline.runtimeBytes, 24407745);
  await main.evaluate((baseline) => {
    const original = window.fetch.bind(window);
    window.__platformTest = {
      provider: 'windowsDpapi',
      hasKey: false,
      fail: false,
      model: baseline,
      writes: 0,
    };
    window.fetch = (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1));
      const fixture = window.__platformTest;
      const response = (value, failed = false) =>
        Promise.resolve(
          new Response(JSON.stringify(value), {
            headers: {
              'Content-Type': 'application/json',
              'Tauri-Response': failed ? 'error' : 'ok',
            },
          }),
        );
      if (command === 'model_status') return response(fixture.model);
      if (command === 'generation_credential_status') {
        if (fixture.fail) return response('PRIVATE_BACKEND_ERROR_SENTINEL', true);
        return response({
          hasKey: fixture.hasKey,
          support: { supported: true, provider: fixture.provider, userBound: true },
        });
      }
      if (['generation_set_key', 'generation_clear_key'].includes(command)) {
        fixture.writes++;
        fixture.hasKey = command === 'generation_set_key';
        return response(null);
      }
      if (['model_download', 'model_start', 'model_import'].includes(command))
        throw Error('Unexpected model action');
      return original(input, options);
    };
  }, baseline);
  await main.getByRole('button', { name: '설정', exact: true }).click();
  const dialog = main.getByRole('dialog', { name: '설정', exact: true });
  await dialog.getByRole('button', { name: 'AI·모델', exact: true }).click();
  const credentials = dialog.locator('.generation-credentials');
  const refresh = credentials.getByRole('button', { name: '키 상태 새로고침', exact: true });
  const save = credentials.getByRole('button', { name: '현재 URL에 키 저장', exact: true });
  for (const [provider, note] of [
    ['windowsDpapi', 'Windows DPAPI'],
    ['macos-keychain', 'macOS 키체인'],
    ['linux-secret-service', 'Secret Service'],
  ]) {
    await main.evaluate((provider) => {
      window.__platformTest.provider = provider;
    }, provider);
    await refresh.click();
    await expect(credentials).toContainText(note);
    await expect(save).toBeEnabled();
    await credentials.getByLabel('저장할 새 API 키', { exact: true }).fill('synthetic-ui-key');
    await save.click();
    await expect(credentials).toContainText('현재 URL에 키를 저장했습니다.');
    await expect(credentials.getByLabel('저장할 새 API 키', { exact: true })).toHaveValue('');
    await credentials.getByRole('button', { name: '현재 URL의 키 삭제', exact: true }).click();
    await expect(credentials).toContainText('현재 URL의 키를 삭제했습니다.');
  }
  await main.evaluate(() => {
    window.__platformTest.fail = true;
  });
  await refresh.click();
  await expect(credentials.getByRole('alert')).toContainText('잠금 해제');
  await expect(credentials).not.toContainText('PRIVATE_BACKEND_ERROR_SENTINEL');
  await expect(save).toBeDisabled();
  await main.evaluate(() => {
    window.__platformTest.fail = false;
  });
  await refresh.click();
  await expect(save).toBeEnabled();
  await credentials.scrollIntoViewIfNeeded();
  await main.screenshot({
    path: join(evidence, 'platform-linux-credentials.png'),
    animations: 'disabled',
  });
  await main.setViewportSize({ width: 390, height: 844 });
  await credentials.scrollIntoViewIfNeeded();
  assert.equal(await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1), true);
  await expect(refresh).toBeVisible();
  await main.screenshot({
    path: join(evidence, 'platform-linux-credentials-mobile.png'),
    animations: 'disabled',
  });
  await main.setViewportSize({ width: 1280, height: 840 });
  await main.evaluate(() => {
    window.__platformTest.provider = 'unknown';
  });
  await refresh.click();
  await expect(credentials).toContainText('지원하지 않습니다.');
  await expect(save).toBeDisabled();

  const model = dialog.locator('.embedding-panel');
  await model.locator('.embedding-files summary').click();
  for (const [target, filename] of [
    ['windows-x86_64', 'llama-b11960-bin-win-cpu-x64.zip'],
    ['macos-aarch64', 'llama-b11960-bin-macos-arm64.tar.gz'],
    ['macos-x86_64', 'llama-b11960-bin-macos-x64.tar.gz'],
    ['linux-x86_64', 'llama-b11960-bin-ubuntu-x64.tar.gz'],
    ['linux-aarch64', 'llama-b11960-bin-ubuntu-arm64.tar.gz'],
  ]) {
    await main.evaluate(
      ({ target, filename }) => {
        window.__platformTest.model = {
          ...window.__platformTest.model,
          runtimeTarget: target,
          runtimeFilename: filename,
        };
      },
      { target, filename },
    );
    await expect(model.locator('.embedding-files')).toContainText(filename);
    await expect(
      model.getByRole('button', { name: '공식 Q8 패키지 다운로드', exact: true }),
    ).toBeEnabled();
  }
  await main.evaluate(() => {
    window.__platformTest.model.runtimeTarget = null;
    window.__platformTest.model.runtimeFilename = null;
  });
  await expect(model).toContainText('공식 런타임이 아직 없습니다.');
  await expect(
    model.getByRole('button', { name: '공식 Q8 패키지 다운로드', exact: true }),
  ).toBeDisabled();
  assert.equal(await main.evaluate(() => window.__platformTest.writes), 6);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      credentialProviders: 3,
      runtimeTargets: 5,
      syntheticPlatformResponses: true,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
