// Own bundled Windows WebView, with synthetic OS-language capabilities only.
// This proves the Mac capability/UI contract, not actual Vision/Quartz execution.
// Browser plugin absent: use the repository's isolated native CDP harness.
import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown } from './harness.mjs';

for (const language of ['ko_KR', 'zh_Hant_HK']) {
  const directory = await mkdtemp(join(tmpdir(), 'prism-ocr-platform-ui-'));
  await writeFile(
    join(directory, 'settings.json'),
    JSON.stringify({
      onboardingVersion: 1,
      theme: language === 'ko_KR' ? 'light' : 'dark',
      startOnLogin: false,
      shortcut: 'Ctrl+Alt+Shift+F10',
      clipboard: { enabled: false, onboarded: true },
      updates: { autoCheck: false },
      ocr: { enabled: false, engine: 'system', language },
      scopes: [],
    }),
  );
  let instance;
  try {
    instance = await launch(
      directory,
      process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
    );
    const { main, invoke, errors } = instance;
    await main.setViewportSize({ width: language === 'ko_KR' ? 1600 : 760, height: 1040 });
    assert.match(main.url(), /^https?:\/\/tauri\.localhost\//);
    assert.match(await main.title(), /Prism/i);
    await main.evaluate(() => {
      const original = window.fetch.bind(window);
      window.__ocrMissing = false;
      window.__ocrModelActions = 0;
      window.__ocrCapabilityCalls = 0;
      window.fetch = (input, options) => {
        const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
        const command =
          url.hostname === 'ipc.localhost' ? decodeURIComponent(url.pathname.slice(1)) : '';
        if (command === 'content_ocr_capabilities') {
          window.__ocrCapabilityCalls++;
          return Promise.resolve(
            new Response(
              JSON.stringify({
                engine: 'macNative',
                available: true,
                pdfAvailable: true,
                reason: null,
                languages: window.__ocrMissing
                  ? [{ tag: 'en-US', name: 'English' }]
                  : [
                      { tag: 'ko-KR', name: '한국어' },
                      { tag: 'zh-Hant', name: '繁體中文' },
                    ],
              }),
              { headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' } },
            ),
          );
        }
        if (['ocr_install', 'model_download', 'model_start'].includes(command)) {
          window.__ocrModelActions++;
          throw Error('Unexpected model action');
        }
        return original(input, options);
      };
    });
    await invoke('open_settings', { tab: 'scopes' });
    const dialog = main.getByRole('dialog', { name: '설정', exact: true });
    const section = dialog.locator('section[aria-labelledby="ocr-settings-title"]');
    const status = section.locator('[role="status"]').first();
    const select = section.getByLabel('OCR 언어', { exact: true });
    await expect(status).toHaveText('macOS Vision');
    await expect(select).toHaveValue(language);
    await status.scrollIntoViewIfNeeded();
    await expect(status).toBeVisible();
    await expect(select).toBeVisible();
    const screenshot = join(directory, 'mac-capability-contract.png');
    await main.screenshot({ path: screenshot });
    await main.evaluate(() => {
      window.__ocrDialog = document.querySelector('[role="dialog"]');
      window.__ocrMissing = true;
    });
    await section.getByRole('button', { name: '다시 확인', exact: true }).click();
    await expect(status).not.toHaveText('macOS Vision');
    await expect(select).toHaveValue(language);
    await main.evaluate(() => {
      window.__ocrMissing = false;
    });
    await section.getByRole('button', { name: '다시 확인', exact: true }).click();
    await expect(status).toHaveText('macOS Vision');
    assert(
      await main.evaluate(() => window.__ocrDialog === document.querySelector('[role="dialog"]')),
    );
    assert.equal(await main.evaluate(() => window.__ocrModelActions), 0);
    const capabilityCalls = await main.evaluate(() => window.__ocrCapabilityCalls);
    assert(capabilityCalls >= 3, 'Initial load and both refreshes must use the capability fixture');
    assert.deepEqual(errors, []);
    console.log(
      JSON.stringify({
        directory,
        language,
        syntheticMacUiContract: true,
        actualMacExecution: false,
        screenshot,
        modelActions: 0,
        capabilityCalls,
        errors: 0,
      }),
    );
  } finally {
    if (instance) await shutdown(instance);
  }
}
