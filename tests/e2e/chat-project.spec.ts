// Actual React client with fixture IPC; no native indexing or inference is claimed.
import { expect, test } from '@playwright/test';
import { defaultSettings, emptySession } from '../../src/lib/types';

declare global {
  interface Window {
    __chatProject: {
      requestedProjects: (string | null)[];
      createdProjects: (string | null)[];
      setProject: (id: string | null) => void;
    };
  }
}

test('a missing chat project never becomes an all-folder conversation', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.addInitScript(
    ({ settings, initialSession }) => {
      let callbackId = 0;
      const callbacks = new Map<number, (event: unknown) => void>();
      const listeners = new Map<string, number>();
      const session = structuredClone(initialSession);
      const fixture = {
        requestedProjects: [] as (string | null)[],
        createdProjects: [] as (string | null)[],
        setProject(projectId: string | null) {
          session.scopeId = projectId;
          const handler = listeners.get('prism:session');
          if (handler) callbacks.get(handler)?.({ payload: structuredClone(session) });
        },
      };
      const status = {
        scanning: false,
        initialScanComplete: true,
        count: 0,
        revision: 0,
        skipped: 0,
        warnings: [],
        watching: false,
      };
      Object.assign(window, {
        __chatProject: fixture,
        isTauri: true,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
        __TAURI_INTERNALS__: {
          metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
          transformCallback(callback: (event: unknown) => void) {
            callbacks.set(++callbackId, callback);
            return callbackId;
          },
          unregisterCallback(id: number) {
            callbacks.delete(id);
          },
          async invoke(command: string, args: Record<string, unknown> = {}) {
            switch (command) {
              case 'plugin:event|listen':
                listeners.set(String(args.event), Number(args.handler));
                return args.handler;
              case 'get_settings':
                return settings;
              case 'get_search_session':
                return session;
              case 'set_search_session':
                Object.assign(session, args.session);
                return;
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
                return { results: [], total: 0, keyboardCorrected: false, status };
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
              case 'rag_list': {
                const project = (args.project as string | null) ?? null;
                fixture.requestedProjects.push(project);
                // Keep the missing project's request pending: old rows must disappear
                // before a replacement reply arrives, without widening to all folders.
                if (project === 'missing') return new Promise(() => {});
                return [
                  {
                    id: 'conversation',
                    title: project ? 'Project conversation' : 'All-folder conversation',
                    createdMs: 1,
                    updatedMs: 1,
                    turnCount: 0,
                    project,
                  },
                ];
              }
              case 'rag_create':
                fixture.createdProjects.push((args.project as string | null) ?? null);
                throw { code: 'storage', message: 'Creation is not part of this fixture' };
              case 'rag_conversation':
                return {
                  conversation: {
                    id: 'conversation',
                    title: 'Fixture conversation',
                    createdMs: 1,
                    updatedMs: 1,
                    turnCount: 0,
                    project: session.scopeId,
                  },
                  turns: [],
                  hasMore: false,
                  selection: null,
                  selectionSet: null,
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
        language: 'en',
        scopes: [
          {
            id: 'project',
            name: 'Project sources',
            path: 'C:\\project',
            filenameEnabled: true,
            contentEnabled: true,
            externalAllowed: false,
          },
          {
            id: 'other',
            name: 'Other sources',
            path: 'C:\\other',
            filenameEnabled: true,
            contentEnabled: true,
            externalAllowed: false,
          },
        ],
        clipboard: { ...defaultSettings.clipboard, enabled: false, onboarded: true },
        updates: { autoCheck: false },
      },
      initialSession: { ...emptySession, scopeId: 'project' },
    },
  );
  await page.goto('/');
  await page
    .getByRole('navigation', { name: 'Prism', exact: true })
    .getByRole('button', { name: 'Chat', exact: true })
    .click();
  await expect(page.getByRole('button', { name: /Project conversation/ })).toBeVisible();
  await page.getByRole('button', { name: /Project conversation/ }).click();
  await expect(
    page.getByRole('heading', { name: 'Fixture conversation', exact: true }),
  ).toBeVisible();
  await page.locator('.chat-retrieval-options > summary').click();
  const sourceChoices = page.locator('.chat-scope-picker input[type="checkbox"]');
  await expect(sourceChoices).toHaveCount(1);
  await expect(sourceChoices.first()).toBeChecked();
  await sourceChoices.first().uncheck();

  await page.evaluate(() => {
    window.__chatProject.setProject('missing');
  });
  await expect(page.locator('.chat-project')).toContainText('Removed folder');
  await expect(page.getByRole('button', { name: /Project conversation/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New conversation', exact: true })).toBeDisabled();
  await expect
    .poll(() => page.evaluate(() => window.__chatProject.requestedProjects))
    .toEqual(['project', 'missing']);
  expect(await page.evaluate(() => window.__chatProject.createdProjects)).toEqual([]);

  await page.getByRole('button', { name: 'Use all folders', exact: true }).click();
  await expect(page.locator('.chat-project')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /All-folder conversation/ })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New conversation', exact: true })).toBeEnabled();
  await expect
    .poll(() => page.evaluate(() => window.__chatProject.requestedProjects))
    .toEqual(['project', 'missing', null]);
  await page.getByRole('button', { name: /All-folder conversation/ }).click();
  await expect(
    page.getByRole('heading', { name: 'Fixture conversation', exact: true }),
  ).toBeVisible();
  await page.locator('.chat-retrieval-options > summary').click();
  await expect(sourceChoices).toHaveCount(2);
  await expect(sourceChoices.nth(0)).toBeChecked();
  await expect(sourceChoices.nth(1)).toBeChecked();
  expect(errors).toEqual([]);
});
