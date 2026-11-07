// Browser plugin not available. Actual client UI; only Tauri IPC is a fixture.
// No model download, runtime execution or native-WebView qualification occurs.
import { expect, test } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defaultSettings, emptySession } from '../../src/lib/types';
import type { EmbeddingStatus } from '../../src/lib/model';

const initial: EmbeddingStatus = {
  phase: 'notInstalled',
  modelId: 'google/embeddinggemma-2',
  modelRevision: 'fixture',
  runtimeVersion: 'b11960',
  runtimeTarget: 'windows-x86_64',
  runtimeFilename: 'llama-b11960-bin-win-cpu-x64.zip',
  runtimeBytes: 24407745,
  quantization: null,
  modelReady: false,
  runtimeReady: false,
  downloadedBytes: 0,
  totalBytes: 0,
  pid: null,
  dimensions: 768,
  lastError: null,
};

for (const late of ['status', 'error'] as const) {
  test(`a late model poll ${late} cannot replace a completed action`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text());
    });
    await page.addInitScript(
      ({ settings, session, initial }) => {
        let callback = 0;
        const fixture = {
          model: initial,
          holdPolls: false,
          polls: [] as {
            resolve: (value: typeof initial) => void;
            reject: (error: Error) => void;
          }[],
          finish: null as null | ((value: typeof initial) => void),
          unexpected: [] as string[],
        };
        Object.assign(window, {
          isTauri: true,
          __modelStatusFixture: fixture,
          __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
          __TAURI_INTERNALS__: {
            metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
            transformCallback: () => ++callback,
            unregisterCallback() {},
            async invoke(command: string, args: Record<string, unknown> = {}) {
              switch (command) {
                case 'plugin:event|listen':
                  return args.handler;
                case 'plugin:event|unlisten':
                  return;
                case 'get_settings':
                  return settings;
                case 'get_search_session':
                  return session;
                case 'runtime_info':
                  return {
                    native: true,
                    platform: 'windows',
                    development: true,
                    installKind: 'isolated',
                    shortcutError: null,
                    startupError: null,
                  };
                case 'index_status':
                  return {
                    scanning: false,
                    initialScanComplete: true,
                    count: 0,
                    revision: 0,
                    skipped: 0,
                    warnings: [],
                    watching: false,
                  };
                case 'content_status':
                  return {
                    phase: 'ready',
                    discovered: 0,
                    indexed: 0,
                    skipped: 0,
                    embedded: 0,
                    revision: 0,
                    currentPath: null,
                    issues: [],
                    embeddingError: null,
                  };
                case 'rag_take_chat_handoff':
                  return null;
                case 'update_status':
                  return {
                    autoCheck: false,
                    checking: false,
                    installing: false,
                    available: null,
                    dismissedVersion: null,
                    canInstall: false,
                    lastCheckMs: null,
                    checkError: null,
                    installError: null,
                    currentVersion: '0.1.0',
                    downloadPage: '',
                    installDisabledReason: 'isolated',
                    stateError: false,
                  };
                case 'generation_credential_status':
                  return {
                    hasKey: false,
                    support: { supported: true, provider: 'windowsDpapi', userBound: true },
                  };
                case 'model_status':
                  if (fixture.holdPolls)
                    return new Promise<typeof initial>((resolve, reject) =>
                      fixture.polls.push({ resolve, reject }),
                    );
                  return structuredClone(fixture.model);
                case 'model_download':
                  fixture.model = {
                    ...initial,
                    phase: 'downloading',
                    totalBytes: 100,
                    downloadedBytes: 50,
                  };
                  fixture.holdPolls = true;
                  return new Promise<typeof initial>((resolve) => {
                    fixture.finish = resolve;
                  });
                default:
                  fixture.unexpected.push(command);
                  throw new Error(`Unconfigured model-status fixture command: ${command}`);
              }
            },
          },
        });
      },
      {
        settings: {
          ...defaultSettings,
          language: 'en',
          onboardingVersion: 1,
          clipboard: { ...defaultSettings.clipboard, enabled: false, onboarded: true },
          updates: { autoCheck: false },
        },
        session: emptySession,
        initial,
      },
    );
    await page.goto('/');
    await expect(page).toHaveTitle('Prism');
    await page.locator('.workspace-tool-bar-right .workspace-tool').first().click();
    const panel = page.getByRole('region', { name: 'Search embedding model', exact: true });
    await expect(panel).toBeVisible();
    const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    const originalDialog = await dialog.elementHandle();
    const preference = dialog.getByRole('checkbox').first();
    await expect(preference).toBeChecked();
    await preference.uncheck();
    await panel.getByRole('button', { name: 'Download official Q8 package', exact: true }).click();
    await expect(panel.locator('.embedding-progress')).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => (window as any).__modelStatusFixture.polls.length))
      .toBeGreaterThan(0);
    await page.evaluate((late) => {
      const fixture = (window as any).__modelStatusFixture;
      const old = fixture.polls.shift();
      const stale = structuredClone(fixture.model);
      fixture.model = {
        ...fixture.model,
        phase: 'ready',
        modelReady: true,
        runtimeReady: true,
        totalBytes: 0,
        downloadedBytes: 0,
      };
      // Both settle in one task, before React can clean up the old poll effect.
      fixture.finish(structuredClone(fixture.model));
      if (late === 'error') old.reject(new Error('Old status read failed'));
      else old.resolve(stale);
    }, late);
    await expect(panel.locator('.embedding-phase')).toHaveAttribute('data-phase', 'ready');
    await expect(panel.locator('.embedding-progress')).toHaveCount(0);
    await expect(panel.getByRole('button', { name: 'Start model', exact: true })).toBeEnabled();
    await expect(panel.locator('.embedding-error')).toHaveCount(0);
    await expect(preference).not.toBeChecked();
    expect(await dialog.evaluate((node, original) => node === original, originalDialog)).toBe(true);
    if (late === 'status') {
      const evidence = join(tmpdir(), 'prism-model-status-evidence');
      await mkdir(evidence, { recursive: true });
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 840 });
        await panel
          .getByRole('heading', { name: 'Search embedding model', exact: true })
          .scrollIntoViewIfNeeded();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        expect(await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
        await expect(panel.locator('.embedding-phase')).toHaveAttribute('data-phase', 'ready');
        await page.screenshot({ path: join(evidence, `fixture-model-ready-${width}.png`) });
      }
    } else {
      // Suppress only obsolete errors. A read from the refreshed generation
      // must still report a real failure and recover through the refresh UI.
      await expect
        .poll(() => page.evaluate(() => (window as any).__modelStatusFixture.polls.length))
        .toBeGreaterThan(0);
      await page.evaluate(() => {
        (window as any).__modelStatusFixture.polls
          .shift()
          .reject(new Error('Current status read failed'));
      });
      await expect(panel.locator('.embedding-phase')).toHaveAttribute('data-phase', 'unknown');
      await expect(panel.getByRole('alert')).toContainText('Current status read failed');
      await expect(
        panel.getByRole('button', { name: 'Download official Q8 package', exact: true }),
      ).toBeDisabled();
      await panel.locator('.embedding-status-heading button').click();
      await expect
        .poll(() => page.evaluate(() => (window as any).__modelStatusFixture.polls.length))
        .toBeGreaterThan(0);
      await page.evaluate(() => {
        const fixture = (window as any).__modelStatusFixture;
        fixture.polls.shift().resolve(structuredClone(fixture.model));
      });
      await expect(panel.locator('.embedding-phase')).toHaveAttribute('data-phase', 'ready');
      await expect(panel.locator('.embedding-error')).toHaveCount(0);
    }
    expect(await page.evaluate(() => (window as any).__modelStatusFixture.unexpected)).toEqual([]);
    expect(errors).toEqual([]);
  });
}
