// Browser plugin not available: production client in Playwright with fixture IPC.
// This verifies editing and IPC payloads, not native persistence or inference.
import { expect, test } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defaultSettings, emptySession } from '../../src/lib/types';
import type { DocumentBlock, DocumentPage, DocumentPlan } from '../../src/lib/documents';

const evidence = join(tmpdir(), 'prism-assembly-editor-ui', `run-${process.pid}-${Date.now()}`);
const sourceScope = {
  id: 'local',
  name: 'Local sources',
  path: 'C:\\sources',
  filenameEnabled: true,
  contentEnabled: true,
  externalAllowed: false,
};
const initial: DocumentPage = {
  document: {
    id: 'doc1',
    title: 'Layout QA',
    createdMs: 1,
    updatedMs: 1,
    currentRevisionId: 'r1',
    revisionCount: 1,
    hasReviewedOutline: false,
  },
  revision: {
    id: 'r1',
    documentId: 'doc1',
    sequence: 1,
    parentRevisionId: null,
    createdMs: 1,
    operation: 'create',
    plan: {
      title: 'Layout QA',
      sections: [
        { id: 'first', heading: 'Overview', instruction: '' },
        { id: 'second', heading: 'Details', instruction: '' },
      ],
    },
    outlineReviewed: false,
    blocks: [],
    markdown: '',
    scopeIds: ['local'],
    scopeSnapshot: {
      revision: 1,
      selectedScopeIds: ['local'],
      scopes: [
        {
          id: 'local',
          name: 'Local sources',
          path: 'C:\\sources',
          contentEnabled: true,
          externalAllowed: false,
        },
      ],
      externalQuestionsAllowed: false,
    },
    citations: [],
    provenance: [],
    externalInputEligible: false,
    ragRequestId: null,
    provider: null,
    requestedModel: null,
    reportedModel: null,
    usage: null,
    claimsVerified: false,
  },
};

test('reviewed field and table editing preserves payloads, validation and responsive layout', async ({
  page,
}) => {
  await mkdir(evidence, { recursive: true });
  const errors: string[] = [];
  const browserWarnings: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (!['error', 'warning'].includes(message.type())) return;
    // These exact warnings come from the response's Permissions-Policy headers,
    // captured in the retained trace, not from application JavaScript.
    if (
      /^Error with Permissions-Policy header: Unrecognized feature: '(?:join-ad-interest-group|run-ad-auction)'\.$/.test(
        message.text(),
      )
    )
      browserWarnings.push(message.text());
    else errors.push(message.text());
  });
  await page.addInitScript(
    ({ initial, settings, session }) => {
      let callback = 0;
      const saved = sessionStorage.getItem('prism.test.assembly-page');
      const persisted = saved
        ? (JSON.parse(saved) as {
            page: DocumentPage;
            history: Record<string, DocumentPage['revision']>;
          })
        : null;
      const fixture = {
        page: persisted?.page ?? initial,
        history: persisted?.history ?? { r1: initial.revision },
        savedPlan: null as DocumentPlan | null,
        unexpected: [] as string[],
      };
      Object.assign(window, {
        isTauri: true,
        __assemblyFixture: fixture,
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
              case 'search_files':
                return {
                  results: [],
                  total: 0,
                  keyboardCorrected: false,
                  status: {
                    scanning: false,
                    initialScanComplete: true,
                    count: 0,
                    revision: 0,
                    skipped: 0,
                    warnings: [],
                    watching: false,
                  },
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
              case 'doc_list':
                return [structuredClone(fixture.page.document)];
              case 'doc_get':
                return structuredClone({
                  document: fixture.page.document,
                  revision:
                    typeof args.revisionId === 'string'
                      ? fixture.history[args.revisionId]
                      : fixture.page.revision,
                });
              case 'doc_validate_access':
                return;
              case 'doc_external_sources':
                return [];
              case 'doc_current_citations':
                return structuredClone(fixture.page.revision.citations);
              case 'doc_job':
                return null;
              case 'doc_revisions':
                return Object.values(fixture.history)
                  .sort((a, b) => b.sequence - a.sequence)
                  .map((revision) => ({
                    id: revision.id,
                    sequence: revision.sequence,
                    parentRevisionId: revision.parentRevisionId,
                    createdMs: revision.createdMs,
                    operation: revision.operation,
                  }));
              case 'doc_save_outline': {
                if (args.expected !== fixture.page.revision.id)
                  throw new Error('Fixture revision conflict');
                fixture.savedPlan = structuredClone(args.plan as DocumentPlan);
                const previous = fixture.page.revision.id;
                const sequence = fixture.page.revision.sequence + 1;
                const id = `r${sequence}`;
                fixture.page = {
                  document: {
                    ...fixture.page.document,
                    currentRevisionId: id,
                    revisionCount: sequence,
                    hasReviewedOutline: true,
                  },
                  revision: {
                    ...fixture.page.revision,
                    id,
                    sequence,
                    parentRevisionId: previous,
                    operation: 'outlineEdit',
                    plan: fixture.savedPlan,
                    outlineReviewed: true,
                    blocks: [],
                  },
                };
                fixture.history[id] = structuredClone(fixture.page.revision);
                sessionStorage.setItem(
                  'prism.test.assembly-page',
                  JSON.stringify({ page: fixture.page, history: fixture.history }),
                );
                return structuredClone(fixture.page.revision);
              }
              default:
                fixture.unexpected.push(command);
                throw new Error(`Unconfigured document-assembly fixture command: ${command}`);
            }
          },
        },
      });
    },
    {
      initial,
      settings: {
        ...defaultSettings,
        onboardingVersion: 1,
        scopes: [sourceScope],
        clipboard: { ...defaultSettings.clipboard, enabled: false, onboarded: true },
        updates: { autoCheck: false },
      },
      session: emptySession,
    },
  );
  const open = async () => {
    await page.goto('/');
    await expect(page).toHaveTitle('Prism');
    await page
      .getByRole('navigation', { name: 'Prism', exact: true })
      .getByRole('button', { name: '문서', exact: true })
      .click();
    await page.locator('.doc-sidebar').getByRole('button').filter({ hasText: 'Layout QA' }).click();
    await page.getByRole('tab', { name: '개요', exact: true }).click();
  };
  await open();
  const mode = page.getByRole('checkbox', { name: '항목·표로 구성', exact: true });
  const review = page.getByRole('checkbox', { name: '개요를 검토했습니다.', exact: true });
  const save = page.getByRole('button', { name: '검토한 개요 저장', exact: true });
  await expect(mode).not.toBeChecked();
  await mode.check();
  await page.getByLabel('누락 안내 언어', { exact: true }).selectOption('en');
  await review.check();
  await expect(save).toBeDisabled();
  const first = page.locator('.doc-outline > ol > li').nth(0);
  const second = page.locator('.doc-outline > ol > li').nth(1);
  await first.getByRole('button', { name: '항목 추가', exact: true }).click();
  await first.getByLabel('항목 이름', { exact: true }).fill('행사 날짜');
  await first.getByLabel('자료에서 찾을 내용', { exact: true }).fill('원문에 기록된 전체 날짜');
  await second.getByRole('button', { name: '두 열 표 추가', exact: true }).click();
  const table = second.locator('.doc-assembly-table');
  await table.getByLabel('항목 이름', { exact: true }).fill('예산');
  await table.getByRole('button', { name: '행 추가', exact: true }).click();
  await table.getByLabel('항목 이름', { exact: true }).nth(1).fill('참가 인원');
  await table
    .locator('.doc-assembly-row')
    .first()
    .getByRole('button', { name: '↓ 아래로 이동', exact: true })
    .click();
  await expect(table.getByLabel('항목 이름', { exact: true }).first()).toHaveValue('참가 인원');
  await second.getByRole('button', { name: '항목 추가', exact: true }).click();
  const note = second.locator('.doc-assembly-field').last();
  await note.getByLabel('항목 이름', { exact: true }).fill('예약 번호');
  await note.getByRole('checkbox', { name: '값을 찾지 못하면 안내 표시', exact: true }).check();
  await review.check();
  await expect(save).toBeEnabled();
  await table.getByLabel('두 번째 열 제목', { exact: true }).fill('항목');
  await expect(review).not.toBeChecked();
  await review.check();
  await expect(save).toBeDisabled();
  await table.getByLabel('두 번째 열 제목', { exact: true }).fill('기록된 값');
  await mode.uncheck();
  await expect(page.locator('.doc-assembly-section')).toHaveCount(0);
  await mode.check();
  await expect(table.getByLabel('두 번째 열 제목', { exact: true })).toHaveValue('기록된 값');
  await expect(note.getByRole('checkbox')).toBeChecked();
  await expect(page.getByLabel('누락 안내 언어', { exact: true })).toHaveValue('en');
  await page.locator('.doc-assembly-controls').scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(evidence, 'overview.png') });
  await table.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(evidence, 'desktop.png') });
  await page.setViewportSize({ width: 760, height: 760 });
  await table.scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const widths = await page
    .locator('.doc-assembly-table')
    .evaluate((node) => ({ width: node.clientWidth, scroll: node.scrollWidth }));
  expect(widths.scroll).toBeLessThanOrEqual(widths.width + 1);
  await page.screenshot({ path: join(evidence, 'narrow.png') });
  await review.check();
  await save.click();
  await expect
    .poll(() => page.evaluate(() => Boolean((window as any).__assemblyFixture.savedPlan)))
    .toBe(true);
  const savedPlan = await page.evaluate(
    () => (window as any).__assemblyFixture.savedPlan as DocumentPlan,
  );
  expect(savedPlan.assembly?.fields.map((field) => field.label)).toEqual([
    '행사 날짜',
    '예산',
    '참가 인원',
    '예약 번호',
  ]);
  expect(savedPlan.assembly?.fields.at(-1)?.missingDisplay).toBe('notice');
  const labels = new Map(savedPlan.assembly!.fields.map((field) => [field.id, field.label]));
  const savedTable = savedPlan.assembly!.sections[1].elements[0];
  expect(savedTable.kind).toBe('fieldTable');
  if (savedTable.kind !== 'fieldTable') throw new Error('Expected reviewed field table');
  expect(savedTable.columns).toEqual(['항목', '기록된 값']);
  expect(savedTable.fields.map((id) => labels.get(id))).toEqual(['참가 인원', '예산']);
  await open();
  await expect(mode).toBeChecked();
  await expect(table.getByLabel('두 번째 열 제목', { exact: true })).toHaveValue('기록된 값');
  await expect(note.getByRole('checkbox')).toBeChecked();
  // A long outline and body share the editor viewport. Reading one pane to
  // its end must not open the other pane with its first content off-screen.
  await page.evaluate(() => {
    const fixture = (window as any).__assemblyFixture;
    fixture.page.revision.blocks = Array.from({ length: 4 }, (_, index): DocumentBlock => ({
      id: `scroll-${index}`,
      sectionId: 'first',
      kind: 'fact',
      markdown: `Visible body ${index + 1}`,
      sourceLabels: [],
      claimsVerified: false,
      userEdited: false,
    }));
    fixture.history[fixture.page.revision.id] = structuredClone(fixture.page.revision);
    sessionStorage.setItem(
      'prism.test.assembly-page',
      JSON.stringify({ page: fixture.page, history: fixture.history }),
    );
  });
  await open();
  const editorScroll = page.locator('.doc-edit-scroll');
  const scrollToEnd = async () => {
    await editorScroll.evaluate((node) => {
      node.scrollTop = node.scrollHeight;
    });
    await expect.poll(() => editorScroll.evaluate((node) => node.scrollTop)).toBeGreaterThan(100);
  };
  await scrollToEnd();
  const bodyTab = page.getByRole('tab', { name: '본문 블록', exact: true });
  await bodyTab.click();
  await expect.poll(() => editorScroll.evaluate((node) => node.scrollTop)).toBe(0);
  await expect(page.locator('.doc-block textarea').first()).toHaveValue('Visible body 1');
  await expect(page.locator('.doc-block textarea').first()).toBeInViewport();
  await page.screenshot({ path: join(evidence, 'body-after-outline.png') });
  await scrollToEnd();
  await bodyTab.press('ArrowLeft');
  await expect(page.getByRole('tab', { name: '개요', exact: true })).toBeFocused();
  await expect.poll(() => editorScroll.evaluate((node) => node.scrollTop)).toBe(0);
  await page.screenshot({ path: join(evidence, 'outline-after-body.png') });
  // A later revision exists; opening the prior structured revision must make
  // every new editing control read-only, just like the existing outline.
  await page.evaluate(() => {
    const fixture = (window as any).__assemblyFixture;
    const previous = fixture.page.revision;
    fixture.page.revision = { ...previous, id: 'r3', sequence: 3, parentRevisionId: previous.id };
    fixture.page.document = { ...fixture.page.document, currentRevisionId: 'r3', revisionCount: 3 };
    fixture.history.r3 = structuredClone(fixture.page.revision);
    sessionStorage.setItem(
      'prism.test.assembly-page',
      JSON.stringify({ page: fixture.page, history: fixture.history }),
    );
  });
  await open();
  const history = page.locator('.doc-history');
  await history.locator('summary').click();
  await history
    .getByRole('button')
    .filter({ hasText: /버전 2 ·/ })
    .click();
  await page.getByRole('tab', { name: '개요', exact: true }).click();
  await expect(mode).toBeDisabled();
  await expect(page.getByLabel('누락 안내 언어', { exact: true })).toBeDisabled();
  await expect(table.getByLabel('두 번째 열 제목', { exact: true })).toBeDisabled();
  await expect(note.getByRole('checkbox')).toBeDisabled();
  await expect(save).toBeDisabled();
  await writeFile(
    join(evidence, 'console.json'),
    JSON.stringify({ appErrors: errors, knownHeaderWarnings: browserWarnings }, null, 2),
  );
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__assemblyFixture.unexpected)).toEqual([]);
  expect(errors).toEqual([]);
});
