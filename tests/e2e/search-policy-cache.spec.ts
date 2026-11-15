// Actual React UI with Tauri IPC fixtures, not an actual native app or index.
// Browser plugin not available: use the existing Playwright browser installation.
import { expect, test } from '@playwright/test';
import { defaultSettings, emptySession, type ClientSettings } from '../../src/lib/types';

declare global {
  interface Window {
    __policyCache: {
      requests: number;
      cancelled: string[];
      releasePrevious: () => void;
      releaseReplacement: () => void;
      settings: () => ClientSettings;
    };
  }
}

for (const mode of ['filename', 'content'] as const) {
  for (const previousPending of [false, true]) {
    test(`${mode} policy change invalidates ${previousPending ? 'pending' : 'displayed'} results without an index revision`, async ({
      page,
    }, info) => {
      const errors: string[] = [];
      const compatibilityNotices: string[] = [];
      const knownNotices = new Set([
        "Error with Permissions-Policy header: Unrecognized feature: 'join-ad-interest-group'.",
        "Error with Permissions-Policy header: Unrecognized feature: 'run-ad-auction'.",
        'Something has shimmed the React DevTools global hook (__REACT_DEVTOOLS_GLOBAL_HOOK__). Fast Refresh is not compatible with this shim and will be disabled.',
      ]);
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (entry) => {
        if (entry.type() !== 'warning' && entry.type() !== 'error') return;
        if (knownNotices.has(entry.text())) compatibilityNotices.push(entry.text());
        else errors.push(entry.text());
      });
      await page.addInitScript(
        ({ settings, session, mode, previousPending }) => {
          let current = structuredClone(settings);
          let callback = 0;
          const fileStatus = {
            scanning: false,
            initialScanComplete: true,
            count: 2,
            revision: 1,
            skipped: 0,
            warnings: [],
            watching: false,
          };
          const contentStatus = {
            phase: 'ready',
            discovered: 2,
            indexed: 2,
            skipped: 0,
            embedded: 0,
            revision: 1,
            currentPath: null,
            issues: [],
            embeddingError: null,
          };
          const state = {
            requests: 0,
            cancelled: [] as string[],
            releasePrevious: () => {},
            releaseReplacement: () => {},
            settings: () => current,
          };
          function response(hidden: boolean) {
            const name = hidden ? 'policycache-hidden.md' : 'policycache-visible.md';
            const path = `C:\\fixture\\${name}`;
            if (mode === 'filename')
              return {
                results: [{ path, name, isDir: false, size: 20, modifiedMs: null }],
                total: 1,
                status: fileStatus,
                keyboardCorrected: false,
              };
            return {
              hits: [
                {
                  chunk: {
                    id: hidden ? 'hidden-chunk' : 'visible-chunk',
                    filePath: path,
                    fingerprint: { sha256: 'a'.repeat(64), sizeBytes: 20, modifiedUnixMs: null },
                    sectionIndex: 0,
                    chunkIndex: 0,
                    text: hidden ? 'policycache private excerpt' : 'policycache public excerpt',
                    page: null,
                    paragraph: 1,
                    heading: null,
                    charStart: 0,
                    charEnd: 25,
                  },
                  score: 1,
                  snippet: hidden ? 'policycache private excerpt' : 'policycache public excerpt',
                },
              ],
              candidateCount: 1,
              candidateLimit: 100,
              hardLimitReached: false,
            };
          }
          Object.assign(window, {
            __policyCache: state,
            isTauri: true,
            __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
            __TAURI_INTERNALS__: {
              metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
              transformCallback: () => ++callback,
              unregisterCallback() {},
              async invoke(command: string, args: Record<string, unknown> = {}) {
                switch (command) {
                  case 'plugin:event|listen':
                    return args.handler;
                  case 'get_settings':
                    return structuredClone(current);
                  case 'get_search_session':
                    return session;
                  case 'save_settings': {
                    if (args.expectedRevision !== current.revision)
                      throw new Error('Stale settings fixture');
                    current = {
                      ...(args.settings as typeof settings),
                      revision: current.revision + 1,
                    };
                    return structuredClone(current);
                  }
                  case 'runtime_info':
                    return {
                      native: true,
                      platform: 'windows',
                      development: true,
                      installKind: 'isolated',
                      shortcutError: null,
                      startupError: null,
                    };
                  // Deliberately frozen: a policy change must not await another poll/event.
                  case 'index_status':
                    return fileStatus;
                  case 'content_status':
                    return contentStatus;
                  case 'search_files':
                  case 'search_content': {
                    const hidden = current.filePolicy[mode].hidden;
                    state.requests++;
                    if (hidden && !previousPending) return response(true);
                    return new Promise((resolve) => {
                      const release = () => resolve(response(hidden));
                      if (hidden) state.releasePrevious = release;
                      else state.releaseReplacement = release;
                    });
                  }
                  case 'content_cancel_search':
                    state.cancelled.push(String(args.requestId));
                    return;
                  case 'index_reset_status':
                    return { lastApplied: false, pending: false, error: null };
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
                  default:
                    return null;
                }
              },
            },
          });
        },
        {
          settings: {
            ...defaultSettings,
            onboardingVersion: 1,
            language: 'ko',
            scopes: [
              {
                id: 'fixture',
                name: 'Fixture',
                path: 'C:\\fixture',
                filenameEnabled: true,
                contentEnabled: true,
                externalAllowed: false,
              },
            ],
            clipboard: { ...defaultSettings.clipboard, enabled: false, onboarded: true },
            updates: { autoCheck: false },
          },
          session: { ...emptySession, query: 'policycache', mode, scopeId: 'fixture' },
          mode,
          previousPending,
        },
      );
      await page.goto('/');
      await expect(page).toHaveTitle('Prism');
      await expect(page.getByRole('textbox', { name: '검색어', exact: true })).toHaveValue(
        'policycache',
      );
      await expect.poll(() => page.evaluate(() => window.__policyCache.requests)).toBe(1);
      const hiddenName = page.getByText('policycache-hidden.md', { exact: true });
      const hiddenExcerpt = page.getByText('policycache private excerpt', { exact: true });
      if (!previousPending) await expect(hiddenName.first()).toBeVisible();
      await page.getByRole('button', { name: '설정', exact: true }).click();
      const modal = page.getByRole('dialog', { name: '설정', exact: true });
      await modal.getByRole('button', { name: '검색 범위', exact: true }).click();
      const target = mode === 'filename' ? '파일명 검색' : '본문·AI 색인';
      await modal
        .locator('.file-policy-settings')
        .getByRole('group', { name: target, exact: true })
        .getByRole('checkbox', { name: '숨김 항목', exact: true })
        .uncheck();
      await modal.getByRole('button', { name: '저장', exact: true }).click();
      await expect(modal).toHaveCount(0);
      await expect(hiddenName).toHaveCount(0);
      await expect(hiddenExcerpt).toHaveCount(0);
      await expect.poll(() => page.evaluate(() => window.__policyCache.requests)).toBe(2);
      await page.evaluate(() => window.__policyCache.releasePrevious());
      if (previousPending && mode === 'content')
        await expect.poll(() => page.evaluate(() => window.__policyCache.cancelled.length)).toBe(1);
      await expect(hiddenName).toHaveCount(0);
      await expect(hiddenExcerpt).toHaveCount(0);
      await page.evaluate(() => window.__policyCache.releaseReplacement());
      await expect(page.getByText('policycache-visible.md', { exact: true }).first()).toBeVisible();
      await expect(hiddenName).toHaveCount(0);
      await expect(hiddenExcerpt).toHaveCount(0);
      const saved = await page.evaluate(() => window.__policyCache.settings());
      expect(saved.filePolicy[mode].hidden).toBe(false);
      expect(saved.filePolicy[mode === 'filename' ? 'content' : 'filename'].hidden).toBe(true);
      expect(saved.scopes[0].externalAllowed).toBe(false);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(errors).toEqual([]);
      await info.attach('compatibility-notices', {
        body: JSON.stringify(compatibilityNotices),
        contentType: 'application/json',
      });
      await page.screenshot({ path: info.outputPath(`${mode}-policy-cache.png`) });
    });
  }
}
