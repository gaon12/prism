// Browser plugin not available: real bundled WebView, SQLite, source reads,
// native PDF renderer and WebP encoder. No API/model/data response replacement.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { deflateSync } from 'node:zlib';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';
const directory = await mkdtemp(join(tmpdir(), 'prism-collections-ui-'));
const profile = join(directory, 'profile');
const originals = join(directory, '원본');
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
function crc(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const name = Buffer.from(type);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const check = Buffer.alloc(4);
  check.writeUInt32BE(crc(Buffer.concat([name, data])));
  return Buffer.concat([length, name, data, check]);
}
function png(color) {
  const width = 80,
    height = 120;
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6;
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const n = y * (width * 4 + 1) + 1 + x * 4;
      raw[n] = color[0];
      raw[n + 1] = color[1];
      raw[n + 2] = color[2];
      raw[n + 3] = 255;
    }
  }
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
const red = png([210, 40, 50]);
const blue = png([30, 80, 200]);
function pdf() {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Count 2 /Kids [3 0 R 5 0 R] >>',
  ];
  for (const rgb of ['1 0 0', '0 0 1']) {
    const stream = `${rgb} rg 0 0 80 120 re f`;
    const streamId = objects.length + 2;
    objects.push(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 80 120] /Resources << >> /Contents ${streamId} 0 R >>`,
    );
    objects.push(`<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);
  }
  let bytes = Buffer.from('%PDF-1.4\n');
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(bytes.length);
    bytes = Buffer.concat([bytes, Buffer.from(`${index + 1} 0 obj\n${object}\nendobj\n`)]);
  }
  const xref = bytes.length;
  return Buffer.concat([
    bytes,
    Buffer.from(
      `xref\n0 ${offsets.length}\n0000000000 65535 f \n` +
        offsets
          .slice(1)
          .map((n) => `${String(n).padStart(10, '0')} 00000 n \n`)
          .join('') +
        `trailer\n<< /Size ${offsets.length} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`,
    ),
  ]);
}
const pdfPath = join(directory, '두 페이지.pdf');
const pdfBytes = pdf();
await writeFile(pdfPath, pdfBytes);
for (let first = 1; first <= 600; first += 24) {
  await Promise.all(
    Array.from({ length: Math.min(24, 601 - first) }, (_, i) =>
      writeFile(join(originals, `${first + i}.png`), first + i === 1 ? red : blue),
    ),
  );
}
let instance;
try {
  instance = await launch(
    profile,
    process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
  );
  const { main, invoke, errors } = instance;
  const settingsBefore = await invoke('get_settings');
  assert.equal((await invoke('runtime_info')).installKind, 'isolated');
  await main.getByRole('button', { name: '컬렉션', exact: true }).click();
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  await expect(region.getByRole('heading', { name: '컬렉션', exact: true })).toBeVisible();
  await region
    .locator('.collection-heading')
    .getByRole('button', { name: '새 컬렉션', exact: true })
    .click();
  let dialog = main.getByRole('dialog', { name: '새 컬렉션', exact: true });
  await dialog.getByLabel('이름', { exact: true }).fill('한국어 갤러리');
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await region
    .locator('.collection-toolbar')
    .getByRole('button', { name: '작품 추가', exact: true })
    .click();
  dialog = main.getByRole('dialog', { name: '작품 추가', exact: true });
  await dialog.getByLabel('이름', { exact: true }).fill('만화 테스트');
  await dialog.getByLabel('종류', { exact: true }).selectOption('comic');
  await dialog.getByLabel('읽기 방향', { exact: true }).selectOption('rightToLeft');
  await dialog.getByLabel('태그 (쉼표로 구분)', { exact: true }).fill('한글, gallery');
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await region
    .locator('.collection-unit-bar')
    .getByRole('button', { name: '권·회차 추가', exact: true })
    .click();
  dialog = main.getByRole('dialog', { name: '권·회차 추가', exact: true });
  await dialog.getByLabel('이름', { exact: true }).fill('제1권');
  await dialog.getByLabel('종류', { exact: true }).selectOption('volume');
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect(region.getByRole('button', { name: '이미지 폴더 추가', exact: true })).toBeEnabled();
  const collection = (await invoke('collection_list', { offset: 0, limit: 100 })).items[0];
  const work = (
    await invoke('collection_works', { collectionId: collection.id, offset: 0, limit: 24 })
  ).items[0];
  let unit = (await invoke('collection_units', { workId: work.id, offset: 0, limit: 100 }))
    .items[0];
  // Observe real DOM insertions before starting real filesystem work. A single
  // immediate isVisible() poll races the first native progress event.
  await region.evaluate((element) => {
    window.__collectionProgressObserved = false;
    window.__collectionProgressObserver = new MutationObserver((records) => {
      for (const record of records)
        for (const node of record.addedNodes)
          if (
            node instanceof Element &&
            (node.matches('[role="progressbar"]') || node.querySelector('[role="progressbar"]'))
          )
            window.__collectionProgressObserved = true;
    });
    window.__collectionProgressObserver.observe(element, { childList: true, subtree: true });
  });
  await main.evaluate(
    ({ unit, path }) => {
      window.__realCollectionImport = window.__TAURI_INTERNALS__
        .invoke('collection_import', { unitId: unit.id, expected: unit.revision, paths: [path] })
        .then(
          (value) => ({ value }),
          (error) => ({ error }),
        );
    },
    { unit, path: originals },
  );
  const imported = await main.evaluate(() => window.__realCollectionImport);
  const progressObserved = await main.evaluate(() => {
    window.__collectionProgressObserver.disconnect();
    return window.__collectionProgressObserved;
  });
  assert.equal(progressObserved, true, 'actual native import displays progress');
  assert.equal(imported.error, undefined);
  unit = imported.value;
  assert.equal(unit.pageCount, 600);
  await until(
    async () => Number(await region.locator('.collection-page-card').count()) === 48,
    'bounded real page grid',
  );
  await expect(region.locator('.collection-page-card').first().locator('img')).toBeVisible();
  const first = (
    await invoke('collection_pages', {
      unitId: unit.id,
      expected: unit.revision,
      offset: 0,
      limit: 48,
    })
  ).items[0];
  assert.equal(first.location.entry, '1.png');
  const media = await invoke('collection_cover', { workId: work.id });
  assert(media.dataUrl.startsWith('data:image/webp;base64,'));
  const pixel = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = 1;
    canvas.height = 1;
    const ctx = canvas.getContext('2d');
    ctx.drawImage(image, 0, 0, 1, 1);
    return [...ctx.getImageData(0, 0, 1, 1).data];
  }, media.dataUrl);
  assert.deepEqual(pixel, [210, 40, 50, 255]);
  await main.screenshot({ path: join(directory, 'pages-ko-light.png') });
  await region
    .locator('.collection-page-card')
    .first()
    .getByRole('button', { name: '1 / 600 페이지', exact: true })
    .click();
  const reader = main.getByRole('dialog', { name: '만화 테스트', exact: true });
  await expect(reader.locator('img')).toBeVisible();
  await reader.press('ArrowLeft');
  await expect(reader.getByText('2 / 600 페이지', { exact: true })).toBeVisible();
  await expect(reader.locator('img')).toBeVisible();
  await main.screenshot({ path: join(directory, 'reader-rtl-ko.png') });
  await until(async () => {
    const next = await invoke('collection_pages', {
      unitId: unit.id,
      expected: unit.revision,
      offset: 1,
      limit: 1,
    });
    return (await invoke('collection_reading', { workId: work.id })).pageId === next.items[0].id;
  }, 'actual reading position persisted');
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  await expect(reader).not.toBeVisible();
  await region.getByRole('button', { name: '이어 읽기', exact: true }).click();
  await expect(reader.getByText('2 / 600 페이지', { exact: true })).toBeVisible();
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  // Moving the last previewed page down must swap with page49, not send it to
  // the end of a book merely because the client previews only48 pages.
  await region
    .locator('.collection-page-card')
    .nth(47)
    .getByRole('button', { name: '뒤로 이동', exact: true })
    .click();
  await until(async () => {
    const current = await invoke('collection_unit_get', { id: unit.id });
    return current.revision > unit.revision;
  }, 'real reorder committed');
  unit = await invoke('collection_unit_get', { id: unit.id });
  const boundary = await invoke('collection_pages', {
    unitId: unit.id,
    expected: unit.revision,
    offset: 47,
    limit: 2,
  });
  assert.equal(boundary.items[0].location.entry, '49.png');
  assert.equal(boundary.items[1].location.entry, '48.png');
  const settingsAfter = await invoke('get_settings');
  assert.deepEqual(settingsAfter.scopes, settingsBefore.scopes);
  assert.equal((await invoke('content_status')).indexed, 0);
  // A changed original cannot reuse the previously verified cached cover.
  await writeFile(join(originals, '1.png'), blue);
  const changed = await main.evaluate(async (workId) => {
    try {
      await window.__TAURI_INTERNALS__.invoke('collection_cover', { workId });
      return null;
    } catch (error) {
      return error.code;
    }
  }, work.id);
  assert.equal(changed, 'sourceChanged');
  await expect(region.getByRole('button', { name: '다시 가져오기', exact: true })).toBeEnabled();
  await region.getByRole('button', { name: '다시 가져오기', exact: true }).click();
  await until(async () => {
    const current = await invoke('collection_unit_get', { id: unit.id });
    return current.state === 'ready' && current.revision > unit.revision;
  }, 'actual reimport');
  const pdfWork = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: 'PDF fixture',
    metadata: {
      kind: 'document',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  let pdfUnit = await invoke('collection_unit_create', {
    workId: pdfWork.id,
    title: 'PDF',
    kind: 'document',
  });
  pdfUnit = await invoke('collection_import', {
    unitId: pdfUnit.id,
    expected: pdfUnit.revision,
    paths: [pdfPath],
  });
  assert.equal(pdfUnit.pageCount, 2);
  const pdfCover = await invoke('collection_cover', { workId: pdfWork.id });
  assert(pdfCover.dataUrl.startsWith('data:image/webp;base64,'));
  const pdfPixel = await main.evaluate(async (url) => {
    const img = new Image();
    img.src = url;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = c.height = 1;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0, 1, 1);
    return [...ctx.getImageData(0, 0, 1, 1).data];
  }, pdfCover.dataUrl);
  assert.deepEqual(pdfPixel, [255, 0, 0, 255]);
  const currentSettings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...currentSettings, language: 'en', theme: 'dark' },
    expectedRevision: currentSettings.revision,
  });
  await main.setViewportSize({ width: 760, height: 700 });
  await expect(main.getByRole('region', { name: 'Collections', exact: true })).toBeVisible();
  assert.equal(await main.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  await main.screenshot({ path: join(directory, 'pages-en-dark-760.png') });
  await main
    .getByRole('region', { name: 'Collections', exact: true })
    .locator('.collection-toolbar')
    .getByRole('button', { name: 'Remove reference', exact: true })
    .click();
  dialog = main.getByRole('dialog', { name: 'Remove reference', exact: true });
  await dialog.getByRole('button', { name: 'Remove reference', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  assert.equal((await invoke('collection_list', { offset: 0, limit: 100 })).total, 0);
  assert.deepEqual(await readFile(join(originals, '1.png')), blue);
  assert.deepEqual(await readFile(join(originals, '600.png')), blue);
  assert.deepEqual(await readFile(pdfPath), pdfBytes);
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      directory,
      pages: 600,
      progressObserved,
      webpPixel: pixel,
      pdfFirstPagePixel: pdfPixel,
      originalsPreserved: true,
      permissionsUnchanged: true,
      errors: errors.length,
    }),
  );
} catch (error) {
  if (instance) {
    await instance.main.screenshot({ path: join(directory, 'failure.png') }).catch(() => {});
    console.log(
      JSON.stringify({
        directory,
        labels: await instance.main
          .locator('dialog[open] label')
          .allInnerTexts()
          .catch(() => []),
      }),
    );
  }
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
