// Browser plugin not available: real bundled WebView and two source-qualified
// app versions. No IPC, document renderer, page list or media response is mocked.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { appendFile, cp, copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

const sha = (data) => createHash('sha256').update(data).digest('hex');
const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE ?? '');
assert(process.env.PRISM_TEST_EXECUTABLE);
const existing = process.env.PRISM_OFFICE_UPGRADE_ROOT;
const directory = existing
  ? resolve(existing)
  : await mkdtemp(join(tmpdir(), 'prism-office-upgrade-'));
const profile = join(directory, 'profile');
const owner = 'Prism Office upgrade QA v1\n';
const expectBlocked = process.env.PRISM_OFFICE_UPGRADE_EXPECT_BLOCKED === '1';
let seed;
if (existing) {
  assert.equal(await readFile(join(directory, 'qa-owner.txt'), 'utf8'), owner);
  seed = JSON.parse(await readFile(join(directory, 'seed.json'), 'utf8'));
} else {
  const old = resolve(process.env.PRISM_OFFICE_UPGRADE_OLD_EXE ?? '');
  assert(process.env.PRISM_OFFICE_UPGRADE_OLD_EXE);
  assert.equal(
    sha(await readFile(old)),
    '68b0ee77ddca18628d8115bab06828eefdbee6358a9ce85f2ba8011c8eee3b6a',
  );
  const engine = resolve(process.env.PRISM_OFFICE_QA_PREPARED_RUNTIME ?? '');
  const source = resolve(process.env.PRISM_OFFICE_QA_TWO_PAGE ?? '');
  assert(process.env.PRISM_OFFICE_QA_PREPARED_RUNTIME && process.env.PRISM_OFFICE_QA_TWO_PAGE);
  assert.equal(
    sha(await readFile(source)),
    'a21f9105ef5d1bc8a9eeaa3688e6d8b91dc385193df5f0c1d4320ee344a7f855',
  );
  const catalog = JSON.parse(await readFile('tools/office-runtime/windows-x64.lock.json', 'utf8'));
  assert.equal(sha(await readFile(join(engine, 'inventory.json'))), catalog.inventorySha256);
  assert.equal(
    await readFile(join(engine, '.prism-office-runtime'), 'utf8'),
    catalog.inventorySha256,
  );
  await writeFile(join(directory, 'qa-owner.txt'), owner);
  await mkdir(join(profile, 'office'), { recursive: true });
  await cp(
    engine,
    join(profile, 'office', `libreoffice-windows-x64-${catalog.inventorySha256.slice(0, 16)}`),
    { recursive: true, force: false, errorOnExist: true },
  );
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
  const original = join(directory, 'two-pages.docx');
  await copyFile(source, original);
  const app = await launch(profile, { executable: old });
  try {
    const { invoke } = app;
    const collection = await invoke('collection_create', {
      title: 'Office 업데이트',
      description: '',
    });
    const work = await invoke('collection_work_create', {
      collectionId: collection.id,
      title: '기존 한글 문서',
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
      title: '순서와 책갈피',
      kind: 'document',
    });
    unit = await invoke('collection_import', {
      unitId: unit.id,
      expected: unit.revision,
      paths: [original],
    });
    const pages = (
      await invoke('collection_pages', {
        unitId: unit.id,
        expected: unit.revision,
        offset: 0,
        limit: 10,
      })
    ).items;
    assert.deepEqual(
      pages.map((p) => p.location.number),
      [1, 2],
    );
    assert(
      pages.every(
        (p) =>
          p.location.renderer ===
          catalog.inventorySha256 +
            ':0edc896a74cf21f432333839a28fc69bef130d792f15437dba511a513d922e71',
      ),
    );
    unit = await invoke('collection_move_page', {
      unitId: unit.id,
      expected: unit.revision,
      pageId: pages[1].id,
      before: pages[0].id,
    });
    await invoke('collection_save_reading', {
      workId: work.id,
      pageId: pages[1].id,
      completed: true,
    });
    const oldCover = await invoke('collection_cover', { workId: work.id });
    assert.equal(oldCover.pageId, pages[1].id);
    const changedOriginal = join(directory, 'changed-two-pages.docx');
    await copyFile(source, changedOriginal);
    let changedUnit = await invoke('collection_unit_create', {
      workId: work.id,
      title: '변경된 원본',
      kind: 'document',
    });
    changedUnit = await invoke('collection_import', {
      unitId: changedUnit.id,
      expected: changedUnit.revision,
      paths: [changedOriginal],
    });
    seed = {
      collection,
      work,
      unit,
      pages,
      original,
      sourceSha256: sha(await readFile(original)),
      oldExecutableSha256: sha(await readFile(old)),
      oldCoverSha256: sha(Buffer.from(oldCover.dataUrl.split(',')[1], 'base64')),
      changedUnit,
      changedOriginal,
    };
    await writeFile(join(directory, 'seed.json'), JSON.stringify(seed, null, 2));
    assert.deepEqual(app.errors, []);
  } finally {
    await shutdown(app);
  }
  await appendFile(seed.changedOriginal, '\nchanged after import\n');
}
assert.equal(sha(await readFile(seed.original)), seed.sourceSha256);
const screenshots = [];
const result = {
  directory,
  installationTest: 'notRunPreparedRuntime',
  candidateSha256: sha(await readFile(candidate)),
  oldExecutableSha256: seed.oldExecutableSha256,
  screenshots,
  expectBlocked,
};
let app;
try {
  app = await launch(profile, { executable: candidate });
  const { main, invoke } = app;
  const warnings = [];
  main.on('console', (m) => {
    if (m.type() === 'warning') warnings.push(m.text());
  });
  await main.setViewportSize({ width: 1600, height: 1050 });
  await main.getByRole('button', { name: '컬렉션', exact: true }).click();
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  await region
    .getByRole('button', { name: new RegExp(seed.work.title) })
    .first()
    .click();
  const screenshot = async (name) => {
    const path = join(directory, name);
    await main.screenshot({ path });
    screenshots.push(path);
  };
  if (expectBlocked) {
    await until(
      async () => (await region.locator('.collection-page-card').count()) === 2,
      'old manifest visible',
    );
    const unit = await invoke('collection_unit_get', { id: seed.unit.id });
    assert.equal(unit.revision, seed.unit.revision);
    // Return the real rejection inside the WebView; Playwright otherwise wraps
    // a rejected non-Error object and loses its native code at the boundary.
    const issue = await main.evaluate(
      (pageId) =>
        window.__TAURI_INTERNALS__.invoke('collection_media', { pageId, thumbnail: false }).then(
          () => null,
          (error) => error,
        ),
      seed.pages[1].id,
    );
    result.blockedIssue = issue;
    assert.equal(issue?.code, 'sourceChanged', JSON.stringify(issue));
    await region
      .locator('.collection-page-card')
      .first()
      .getByRole('button', { name: '1 / 2 페이지', exact: true })
      .click();
    await expect(main.getByRole('dialog').getByRole('alert')).toBeVisible();
    await screenshot('before-upgrade-blocked.png');
    result.expectedFailureObserved = true;
  } else {
    await expect(
      region.getByRole('progressbar', { name: 'Office 미리보기를 업데이트하고 있습니다.' }),
    ).toBeVisible({ timeout: 30_000 });
    await main.evaluate(() => {
      window.__upgradeUnitBar = document.querySelector('.collection-unit-bar');
    });
    await screenshot('updating-existing-preview.png');
    let unit;
    await until(
      async () => {
        unit = await invoke('collection_unit_get', { id: seed.unit.id });
        return unit.revision > seed.unit.revision;
      },
      'automatic renderer migration',
      150_000,
    );
    const pages = (
      await invoke('collection_pages', {
        unitId: unit.id,
        expected: unit.revision,
        offset: 0,
        limit: 10,
      })
    ).items;
    assert.deepEqual(
      pages.map((p) => p.location.number),
      [2, 1],
    );
    assert(
      pages.every(
        (p) =>
          p.sourceHash === seed.pages[0].sourceHash && !seed.pages.some((old) => old.id === p.id),
      ),
    );
    const renderer = await readFile('tools/office-runtime/windows-x64.lock.json', 'utf8');
    const fp =
      JSON.parse(renderer).inventorySha256 +
      ':' +
      sha(await readFile('tools/office-runtime/worker.py'));
    assert(pages.every((p) => p.location.renderer === fp));
    const reading = await invoke('collection_reading', { workId: seed.work.id });
    assert.equal(reading.pageId, pages[0].id);
    assert.equal(reading.completed, true);
    assert.equal(
      await main.evaluate(
        () => window.__upgradeUnitBar === document.querySelector('.collection-unit-bar'),
      ),
      true,
    );
    result.unitToolbarStable = true;
    await expect(region.locator('.collection-page-card').first().locator('img')).toBeVisible({
      timeout: 90_000,
    });
    await screenshot('after-upgrade-gallery.png');
    await region
      .locator('.collection-page-card')
      .first()
      .getByRole('button', { name: '1 / 2 페이지', exact: true })
      .click();
    const reader = main.getByRole('dialog', { name: seed.work.title, exact: true });
    await expect(reader.locator('img')).toBeVisible();
    await screenshot('after-upgrade-reader.png');
    await reader.getByRole('button', { name: '닫기', exact: true }).click();
    await main.setViewportSize({ width: 1024, height: 768 });
    await screenshot('after-upgrade-1024.png');
    assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    result.unit = unit;
    result.pages = pages;
    result.bookmarkPreserved = true;
    result.orderPreserved = true;
    await main.evaluate(async () => {
      window.__upgradeEvents = [];
      const handler = window.__TAURI_INTERNALS__.transformCallback((event) =>
        window.__upgradeEvents.push(event.payload),
      );
      window.__upgradeListener = await window.__TAURI_INTERNALS__.invoke('plugin:event|listen', {
        event: 'prism:collections',
        target: { kind: 'Any' },
        handler,
      });
    });
    await region.locator('.collection-unit-bar select').selectOption(seed.changedUnit.id);
    await expect(region.getByRole('alert')).toContainText('원본');
    await expect(region.locator('.collection-page-card')).toHaveCount(0);
    await expect(region.getByRole('button', { name: '읽기', exact: true })).toBeDisabled();
    await until(
      async () => (await invoke('collection_jobs')).length === 0,
      'failed preparation cleared its job',
    );
    const before = await main.evaluate(
      (id) =>
        window.__upgradeEvents.filter(
          (event) => event.unitId === id && event.phase === 'prepareFailed',
        ).length,
      seed.changedUnit.id,
    );
    assert(before > 0);
    await main.waitForTimeout(750);
    assert.equal(
      await main.evaluate(
        (id) =>
          window.__upgradeEvents.filter(
            (event) => event.unitId === id && event.phase === 'prepareFailed',
          ).length,
        seed.changedUnit.id,
      ),
      before,
    );
    const unchanged = await invoke('collection_unit_get', { id: seed.changedUnit.id });
    assert.equal(unchanged.revision, seed.changedUnit.revision);
    await screenshot('changed-original-rejected.png');
    result.changedOriginalRejected = true;
    result.failedPreparationDidNotLoop = true;
    const retry = region.getByRole('button', { name: '다시 가져오기', exact: true });
    await expect(retry).toBeVisible();
    // Restore only this test-owned original, then exercise explicit reimport.
    // A failed automatic preparation must leave these recovery controls usable.
    await copyFile(seed.original, seed.changedOriginal);
    await retry.click();
    await until(
      async () =>
        (await invoke('collection_unit_get', { id: seed.changedUnit.id })).revision >
        seed.changedUnit.revision,
      'explicit reimport after failed preparation',
      150_000,
    );
    await expect(region.getByRole('alert')).toHaveCount(0);
    await expect(region.locator('.collection-page-card').first().locator('img')).toBeVisible();
    await screenshot('manual-reimport-restored.png');
    result.explicitReimportAvailable = true;
    result.passed = true;
  }
  assert.equal(sha(await readFile(seed.original)), seed.sourceSha256);
  assert.deepEqual(app.errors, []);
  assert.deepEqual(warnings, []);
  result.consoleErrors = app.errors;
  result.consoleWarnings = warnings;
} catch (error) {
  result.error = String(error.stack ?? error);
  throw error;
} finally {
  await writeFile(
    join(directory, expectBlocked ? 'baseline.json' : 'upgrade-result.json'),
    JSON.stringify(result, null, 2),
  );
  await shutdown(app);
  console.log(JSON.stringify(result));
}
