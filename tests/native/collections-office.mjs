// Browser plugin not available: use the real bundled WebView, verified engine,
// SQLite, native PDF rasterizer and WebP encoder. No renderer/API replacement.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

const archive = resolve(process.env.PRISM_OFFICE_QA_ARCHIVE ?? '');
const twoPage = resolve(process.env.PRISM_OFFICE_QA_TWO_PAGE ?? '');
assert(
  process.env.PRISM_OFFICE_QA_ARCHIVE && process.env.PRISM_OFFICE_QA_TWO_PAGE,
  'Supply the pinned local runtime archive and the qualified synthetic two-page DOCX.',
);
const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
assert.equal(
  sha(await readFile(archive)),
  '79dac4566699d5df01bd0ec3b8c830f1832aebb2ef739ebe917df1d60c0d09ab',
);
assert.equal(
  sha(await readFile(twoPage)),
  'a21f9105ef5d1bc8a9eeaa3688e6d8b91dc385193df5f0c1d4320ee344a7f855',
);
const directory = await mkdtemp(join(tmpdir(), 'prism-collection-office-'));
const profile = join(directory, 'profile');
const originals = join(directory, 'originals');
await mkdir(profile);
await mkdir(originals);
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
const docx = join(originals, 'original.docx');
const korean = join(originals, 'korean-two-pages.docx');
await copyFile('tests/fixtures/encrypted-office/example.docx', docx);
await copyFile(twoPage, korean);
const originalHashes = [sha(await readFile(docx)), sha(await readFile(korean))];
let instance;
const result = { directory, screenshots: [], phase: 'launch' };
try {
  instance = await launch(
    profile,
    process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
  );
  const { main, invoke, errors } = instance;
  const warnings = [];
  for (const page of [main, instance.quick])
    page.on('console', (message) => {
      if (message.type() === 'warning') warnings.push(message.text());
    });
  const settings = await invoke('get_settings');
  assert.equal((await invoke('runtime_info')).installKind, 'isolated');
  const status = await invoke('collection_office_status');
  assert.equal(status.supported, true);
  assert.equal(status.installed, false);
  assert.equal(status.downloadAvailable, false);
  const collection = await invoke('collection_create', {
    title: 'Office 원본 페이지',
    description: '',
  });
  const work = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: '한글 원본 문서',
    metadata: {
      kind: 'document',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  let unit = await invoke('collection_unit_create', {
    workId: work.id,
    title: '두 페이지 문서',
    kind: 'document',
  });
  unit = await invoke('collection_import', {
    unitId: unit.id,
    expected: unit.revision,
    paths: [korean],
  });
  const pageList = () =>
    invoke('collection_pages', { unitId: unit.id, expected: unit.revision, offset: 0, limit: 10 });
  assert((await pageList()).items.every((page) => page.location.kind === 'text'));

  // Cancel a real archive installation, then verify only its private stage was
  // removed. Do not touch an installed system engine or a normal app profile.
  result.phase = 'cancel-install';
  await main.evaluate((path) => {
    window.__officeInstall = window.__TAURI_INTERNALS__
      .invoke('collection_office_install', { path })
      .then(
        () => ({ ok: true }),
        (error) => ({ error }),
      );
  }, archive);
  await until(
    async () => (await invoke('collection_office_status')).installing,
    'real runtime installation starts',
  );
  await invoke('collection_office_cancel');
  assert.equal((await main.evaluate(() => window.__officeInstall)).error.code, 'cancelled');
  assert.equal((await invoke('collection_office_status')).installed, false);
  const stages = await readdir(join(profile, 'office')).catch((error) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  assert.deepEqual(stages, []);
  result.installCancelled = true;
  result.phase = 'install';
  // Tauri rejects with a plain DTO. Return it across the WebView boundary so
  // Playwright cannot reduce the real installation failure to "Object".
  result.installOutcome = await main.evaluate(
    (path) =>
      window.__TAURI_INTERNALS__.invoke('collection_office_install', { path }).then(
        () => ({ ok: true }),
        (error) => ({ error }),
      ),
    archive,
  );
  result.installStatus = await invoke('collection_office_status');
  assert.deepEqual(result.installOutcome, { ok: true });
  assert.equal((await invoke('collection_office_status')).installed, true);
  assert.equal((await invoke('collection_office_status')).installing, false);
  result.installed = true;

  result.phase = 'gallery';
  await main.getByRole('button', { name: '컬렉션', exact: true }).click();
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  await region.getByRole('button', { name: work.title, exact: true }).click();
  // Wait for the selected unit, not merely the work-card click, before saving
  // the node identity. A null initial reference does not test flickering.
  await expect(region.locator('.collection-unit-bar')).toBeVisible();
  // Existing data changes from an explicitly labelled text preview only when
  // the user explicitly rebuilds the unit's manifest with the installed engine.
  await main.evaluate((unit) => {
    window.__officeUnitBar = document.querySelector('.collection-unit-bar');
    if (!window.__officeUnitBar) throw new Error('The unit toolbar was not mounted');
    window.__officePageImport = window.__TAURI_INTERNALS__
      .invoke('collection_import', {
        unitId: unit.id,
        expected: unit.revision,
        paths: [],
      })
      .then(
        (value) => ({ value }),
        (error) => ({ error }),
      );
  }, unit);
  await expect(region.getByRole('progressbar')).toBeVisible();
  const imported = await main.evaluate(() => window.__officePageImport);
  assert.equal(imported.error, undefined, JSON.stringify(imported.error));
  unit = imported.value;
  assert.equal(
    await main.evaluate(
      () => window.__officeUnitBar === document.querySelector('.collection-unit-bar'),
    ),
    true,
  );
  result.unitToolbarStable = true;
  assert.equal(unit.pageCount, 2);
  const pages = (await pageList()).items;
  assert.deepEqual(
    pages.map((page) => page.location.kind),
    ['office', 'office'],
  );
  assert.deepEqual(
    pages.map((page) => page.location.number),
    [1, 2],
  );
  assert.notEqual(pages[0].id, pages[1].id);
  assert(pages.every((page) => /^[a-f0-9]{64}:[a-f0-9]{64}$/.test(page.location.renderer)));
  const cover = await invoke('collection_cover', { workId: work.id });
  assert.equal(cover.pageId, pages[0].id);
  assert(cover.dataUrl.startsWith('data:image/webp;base64,'));
  assert.equal(cover.text, null);
  const second = await invoke('collection_media', { pageId: pages[1].id, thumbnail: true });
  assert.notEqual(cover.dataUrl, second.dataUrl);
  await expect(region.locator('.collection-page-card')).toHaveCount(2);
  await expect(region.locator('.collection-page-card').first().locator('img')).toBeVisible();
  const screenshot = async (name) => {
    const path = join(directory, name);
    await main.screenshot({ path });
    result.screenshots.push(path);
  };
  await screenshot('office-gallery-ko-light.png');
  await region
    .locator('.collection-page-card')
    .first()
    .getByRole('button', { name: '1 / 2 페이지', exact: true })
    .click();
  const reader = main.getByRole('dialog', { name: work.title, exact: true });
  await expect(reader.locator('img')).toBeVisible();
  assert.equal(await reader.locator('.collection-text-label').count(), 0);
  await expect(reader.getByText('1 / 2 페이지', { exact: true })).toBeVisible();
  await screenshot('office-first-page-ko.png');
  await reader.press('ArrowRight');
  await expect(reader.getByText('2 / 2 페이지', { exact: true })).toBeVisible();
  await expect(reader.locator('img')).toBeVisible();
  await screenshot('office-second-page-ko.png');
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  await until(
    async () => (await invoke('collection_reading', { workId: work.id })).pageId === pages[1].id,
    'original second-page bookmark',
  );
  await main.setViewportSize({ width: 1024, height: 768 });
  await screenshot('office-gallery-1024.png');
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);

  assert.deepEqual([sha(await readFile(docx)), sha(await readFile(korean))], originalHashes);
  assert.deepEqual((await invoke('get_settings')).scopes, settings.scopes);
  assert.equal((await invoke('content_status')).indexed, 0);
  result.phase = 'source-changed';
  await writeFile(korean, 'changed original');
  for (const thumbnail of [true, false]) {
    // Normalize the real Tauri rejection inside its WebView: Playwright wraps
    // a cross-process non-Error rejection as Error("Object"), losing code.
    const issue = await main.evaluate(
      async (args) => {
        try {
          await window.__TAURI_INTERNALS__.invoke('collection_media', args);
          return null;
        } catch (error) {
          return error;
        }
      },
      { pageId: pages[0].id, thumbnail },
    );
    assert.equal(issue?.code, 'sourceChanged', JSON.stringify(issue));
  }
  await region
    .locator('.collection-page-card')
    .first()
    .getByRole('button', { name: '1 / 2 페이지', exact: true })
    .click();
  await expect(reader.getByRole('alert')).toBeVisible();
  assert.equal(await reader.locator('img').count(), 0);
  await screenshot('office-source-changed.png');
  assert.equal(sha(await readFile(docx)), originalHashes[0]);
  assert.deepEqual(errors, []);
  assert.deepEqual(warnings, []);
  result.passed = true;
  result.phase = 'passed';
  result.pages = pages.map(({ id, location }) => ({ id, location }));
  result.runtime = await invoke('collection_office_status');
  result.consoleErrors = errors;
  result.consoleWarnings = warnings;
  result.sourceChangeRejected = true;
  console.log(JSON.stringify(result));
} catch (error) {
  result.error = error instanceof Error ? error.message : error;
  if (instance) {
    result.consoleErrors = instance.errors;
    await instance.main.screenshot({ path: join(directory, 'failure.png') }).catch(() => {});
  }
  throw error;
} finally {
  if (instance) await shutdown(instance);
  await writeFile(join(directory, 'result.json'), JSON.stringify(result, null, 2));
}
