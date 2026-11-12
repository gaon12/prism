// Actual client UI; only Tauri IPC is a fixture (no native app or index).
// The filename result list draws one page of rows and adds pages as its end
// comes into view, wherever the scrolling container is.
import { expect, test } from '@playwright/test';
import { defaultSettings, emptySession } from '../../src/lib/types';

test('filename results draw a page at a time and follow the keyboard selection', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.addInitScript(
    ({ settings, session }) => {
      let callback = 0;
      const status = {
        scanning: false,
        initialScanComplete: true,
        count: 200,
        revision: 1,
        skipped: 0,
        warnings: [],
        watching: false,
      };
      // 200 rows, the most one search returns.
      const results = Array.from({ length: 200 }, (_, index) => ({
        path: `C:\\fixture\\report-${String(index).padStart(3, '0')}.md`,
        name: `report-${String(index).padStart(3, '0')}.md`,
        isDir: false,
        size: 1024 + index,
        modifiedMs: 1_700_000_000_000,
      }));
      Object.assign(window, {
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
                return status;
              case 'search_files':
                return { results, total: 1200, status, keyboardCorrected: false };
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
                // Everything else this screen asks for is not needed here.
                return null;
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
        scopes: [
          {
            id: 'fixture',
            name: 'fixture',
            path: 'C:\\fixture',
            filenameEnabled: true,
            contentEnabled: false,
            externalAllowed: false,
          },
        ],
        clipboard: { ...defaultSettings.clipboard, enabled: false, onboarded: true },
        updates: { autoCheck: false },
      },
      session: { ...emptySession, mode: 'filename', query: 'report' },
    },
  );
  await page.setViewportSize({ width: 1280, height: 840 });
  await page.goto('/');
  const list = page.getByRole('listbox', { name: 'Search results', exact: true });
  const rows = list.getByRole('option');
  await expect(rows.first()).toBeVisible();
  // One page plus at most the pages needed to fill the window; never all.
  // This checks that drawing settles: an earlier version kept adding pages
  // render after render (the list is not what scrolls), so give it time.
  await page.waitForTimeout(1500);
  const first = await rows.count();
  expect(first).toBeGreaterThanOrEqual(40);
  expect(first).toBeLessThan(200);
  await expect(rows.first()).toHaveAttribute('aria-setsize', '200');
  await expect(rows.first()).toHaveAttribute('aria-posinset', '1');

  // Scrolling the last drawn row into view draws another page.
  await rows.last().scrollIntoViewIfNeeded();
  await expect.poll(() => rows.count()).toBeGreaterThan(first);

  // ArrowUp from the first row wraps to row 200, which is drawn and selected.
  await rows.first().click();
  await list.press('ArrowUp');
  await expect(rows).toHaveCount(200);
  await expect(list.locator('[aria-selected="true"]')).toHaveAttribute('aria-posinset', '200');
  expect(errors).toEqual([]);
});
