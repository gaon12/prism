// Browser plugin absent: own bundled WebView and real EPUB/SQLite/cache IPC.
// No fake media, renderer, API response, model or document content is injected.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-epub-native-'));
const profile = join(directory, 'profile');
await mkdir(profile);
const original = await readFile(resolve('tests/fixtures/epub/spine.epub'));
const path = join(directory, '원문 소설.epub');
await writeFile(path, original);
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F10',
    language: 'ko',
    theme: 'light',
    scopes: [],
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
  }),
);
// This is the fixture's explicit local tracker address. It must receive zero
// requests; it is never a real provider or a replacement for file/media IPC.
const requests = [];
const tracker = createServer((request, response) => {
  requests.push(request.url);
  response.writeHead(200);
  response.end('unexpected');
});
await new Promise((done, reject) => {
  tracker.once('error', reject);
  tracker.listen(38491, '127.0.0.1', done);
});
let instance;
try {
  const options = process.env.PRISM_TEST_EXECUTABLE
    ? { executable: process.env.PRISM_TEST_EXECUTABLE }
    : {};
  instance = await launch(profile, options);
  let { main, invoke } = instance;
  const before = await invoke('get_settings');
  const collection = await invoke('collection_create', {
    title: 'EPUB 원문 책장',
    description: '',
  });
  const work = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: 'EPUB 원문 테스트',
    metadata: {
      kind: 'novel',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  let unit = await invoke('collection_unit_create', {
    workId: work.id,
    title: '첫 권',
    kind: 'volume',
  });
  unit = await invoke('collection_import', {
    unitId: unit.id,
    expected: unit.revision,
    paths: [path],
  });
  const svgPath = join(directory, 'SVG 원문.epub');
  const svgOriginal = await readFile(resolve('tests/fixtures/epub/svg.epub'));
  await writeFile(svgPath, svgOriginal);
  const svgWork = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: 'SVG 첫 페이지',
    metadata: {
      kind: 'comic',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  const svgUnit = await invoke('collection_unit_create', {
    workId: svgWork.id,
    title: 'SVG 권',
    kind: 'volume',
  });
  await invoke('collection_import', {
    unitId: svgUnit.id,
    expected: svgUnit.revision,
    paths: [svgPath],
  });
  const svgBefore = await invoke('collection_cover', { workId: svgWork.id });
  assert.equal(svgBefore.document.width, 200);
  assert.equal(svgBefore.document.height, 300);
  const pages = (
    await invoke('collection_pages', {
      unitId: unit.id,
      expected: unit.revision,
      offset: 0,
      limit: 48,
    })
  ).items;
  assert.equal(pages.length, 2);
  assert.equal(pages[0].location.entry, 'OPS/chapters/2-first.xhtml');
  assert.equal(pages[1].location.entry, 'OPS/chapters/10-second.xhtml');
  const first = await invoke('collection_cover', { workId: work.id });
  assert.equal(first.pageId, pages[0].id);
  assert.equal(first.sourceHash, digest(original));
  assert(first.document && !first.dataUrl);
  assert(!first.document.html.includes('127.0.0.1') && !first.document.html.includes('<script'));
  await main.getByRole('button', { name: '컬렉션', exact: true }).click();
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  const card = region.getByRole('button', { name: work.title, exact: true });
  await expect(card.locator('img')).toBeVisible({ timeout: 25_000 });
  const cover = await invoke('collection_cover', { workId: work.id });
  assert(cover.dataUrl?.startsWith('data:image/webp;base64,') && !cover.document);
  await expect(
    region.getByRole('button', { name: svgWork.title, exact: true }).locator('img'),
  ).toBeVisible();
  const svgCover = await invoke('collection_cover', { workId: svgWork.id });
  const svgPixel = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    return {
      width: image.width,
      height: image.height,
      rgba: [...context.getImageData(100, 150, 1, 1).data],
    };
  }, svgCover.dataUrl);
  assert.equal(svgPixel.width, 200);
  assert.equal(svgPixel.height, 300);
  for (const [i, color] of [210, 40, 50].entries()) assert(Math.abs(svgPixel.rgba[i] - color) <= 8);
  const pixel = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    return {
      width: image.width,
      height: image.height,
      rgba: [
        ...context.getImageData(Math.floor(image.width / 2), Math.floor(image.height / 2), 1, 1)
          .data,
      ],
    };
  }, cover.dataUrl);
  assert.equal(pixel.width, 360);
  assert.equal(pixel.height, 480);
  for (const [i, color] of [210, 40, 50].entries())
    assert(
      Math.abs(pixel.rgba[i] - color) <= 8,
      `Original first-page pixels, not green declared cover: ${JSON.stringify(pixel)}`,
    );
  await card.click();
  await expect(region.locator('.collection-page-card')).toHaveCount(2);
  await expect(region.locator('.collection-page-card').nth(1).locator('img')).toBeVisible();
  const textThumb = await invoke('collection_media', { pageId: pages[1].id, thumbnail: true });
  assert(textThumb.dataUrl && !textThumb.document);
  const textPixels = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    let ink = 0,
      white = 0;
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] < 150 && pixels[i + 1] < 150 && pixels[i + 2] < 150) ink++;
      if (pixels[i] > 245 && pixels[i + 1] > 245 && pixels[i + 2] > 245) white++;
    }
    return { ink, white };
  }, textThumb.dataUrl);
  assert(
    textPixels.ink > 200 && textPixels.white > 20_000,
    'Actual text/table ink on a white original page',
  );
  await writeFile(
    join(directory, 'epub-text-first-viewport.webp'),
    Buffer.from(textThumb.dataUrl.split(',')[1], 'base64'),
  );
  await region.getByRole('button', { name: '읽기', exact: true }).click();
  let reader = main.locator('dialog.collection-reader');
  let frame = main.frameLocator('.collection-epub iframe');
  await expect(
    frame.getByRole('heading', { name: '첫 번째 장 — 原文', exact: true }),
  ).toBeVisible();
  assert.equal(await main.evaluate(() => Boolean(window.__epubInjected)), false);
  assert.equal(await frame.locator('script,iframe,object,form').count(), 0);
  assert.equal(await reader.locator('iframe').getAttribute('sandbox'), 'allow-same-origin');
  const pageFits = () =>
    reader.evaluate((element) => {
      const viewport = element.querySelector('.collection-reader-page');
      const frame = element.querySelector('iframe');
      return frame.getBoundingClientRect().bottom <= viewport.getBoundingClientRect().bottom - 15;
    });
  await until(pageFits, 'fixed EPUB page fits available height');
  const fittedHeight = (await reader.locator('iframe').boundingBox()).height;
  await reader.getByRole('button', { name: '너비 맞춤', exact: true }).click();
  await until(
    async () => (await reader.locator('iframe').boundingBox()).height > fittedHeight + 5,
    'fit width changes actual page scale',
  );
  await reader.getByRole('button', { name: '한 페이지 맞춤', exact: true }).click();
  await until(pageFits, 'fit-page restoration');
  const firstScreenshot = join(directory, 'epub-first-page-light.png');
  await main.screenshot({ path: firstScreenshot });
  // Keyboard focus inside the frame must still reach the actual reader.
  await frame.getByRole('heading', { name: '첫 번째 장 — 原文', exact: true }).click();
  await main.keyboard.press('ArrowRight');
  frame = main.frameLocator('.collection-epub iframe');
  await expect(
    frame.getByRole('heading', { name: '두 번째 장 — Table and Ruby', exact: true }),
  ).toBeVisible();
  await expect(frame.getByRole('cell', { name: '책', exact: true })).toBeVisible();
  await expect(frame.locator('ruby')).toHaveText('漢字かんじ');
  assert.equal(
    await frame
      .getByRole('cell', { name: '책', exact: true })
      .evaluate((element) => getComputedStyle(element).borderTopStyle),
    'solid',
  );
  await until(
    async () => (await invoke('collection_reading', { workId: work.id })).pageId === pages[1].id,
    'saved original spine position',
  );
  await frame.getByRole('heading', { name: '두 번째 장 — Table and Ruby', exact: true }).click();
  await main.keyboard.press('Escape');
  await expect(reader).not.toBeVisible();
  const current = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...current, theme: 'dark' },
    expectedRevision: current.revision,
  });
  await main.setViewportSize({ width: 760, height: 900 });
  await region.getByRole('button', { name: '이어 읽기', exact: true }).click();
  reader = main.locator('dialog.collection-reader');
  frame = main.frameLocator('.collection-epub iframe');
  await expect(
    frame.getByRole('heading', { name: '두 번째 장 — Table and Ruby', exact: true }),
  ).toBeVisible();
  await frame
    .getByText('이어지는 원문 단락 100 — English 日本語 한글.', { exact: true })
    .scrollIntoViewIfNeeded();
  await expect(
    frame.getByText('이어지는 원문 단락 100 — English 日本語 한글.', { exact: true }),
  ).toBeVisible();
  await frame
    .getByRole('heading', { name: '두 번째 장 — Table and Ruby', exact: true })
    .scrollIntoViewIfNeeded();
  assert(
    await reader
      .locator('.collection-reader-page')
      .evaluate((element) => element.scrollHeight <= element.clientHeight + 1),
    'Reflow chapter has one scrolling viewport',
  );
  const secondScreenshot = join(directory, 'epub-second-page-dark760.png');
  await main.screenshot({ path: secondScreenshot });
  assert(await reader.evaluate((element) => element.scrollWidth <= element.clientWidth + 1));
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  const policyDenials = instance.errors.filter(
    (message) =>
      message ===
      "Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set.",
  );
  // Native document-start injections are denied by the unscripted frame.
  // Keep these exact expected sandbox denials separate from application errors.
  assert.deepEqual(
    instance.errors.filter((message) => !policyDenials.includes(message)),
    [],
  );
  await shutdown(instance);
  instance = null;
  instance = await launch(profile, options);
  ({ main, invoke } = instance);
  const expectNativeError = async (command, args, code) => {
    const result = await main.evaluate(
      async ({ command, args }) => {
        try {
          return { value: await window.__TAURI_INTERNALS__.invoke(command, args) };
        } catch (error) {
          return { error };
        }
      },
      { command, args },
    );
    assert(result.error, `${command} must reject`);
    if (code) assert.equal(result.error.code, code);
  };
  const reopened = await invoke('collection_cover', { workId: work.id });
  assert.equal(reopened.dataUrl, cover.dataUrl);
  assert.equal(reopened.document, null);
  assert.equal((await invoke('collection_reading', { workId: work.id })).pageId, pages[1].id);
  await writeFile(path, 'changed owned fixture');
  await expectNativeError('collection_cover', { workId: work.id }, 'sourceChanged');
  await expectNativeError(
    'collection_cache_epub',
    {
      pageId: pages[0].id,
      sourceHash: pages[0].sourceHash,
      dataUrl: cover.dataUrl,
    },
    'sourceChanged',
  );
  await assert.rejects(
    invoke('collection_import', { unitId: unit.id, expected: unit.revision, paths: [] }),
  );
  assert.equal((await invoke('collection_unit_get', { id: unit.id })).pageCount, 2);
  assert.equal((await invoke('collection_reading', { workId: work.id })).pageId, pages[1].id);
  await writeFile(path, original);
  assert.equal(digest(await readFile(path)), digest(original));
  assert.equal(digest(await readFile(svgPath)), digest(svgOriginal));
  const after = await invoke('get_settings');
  assert.deepEqual(after.scopes, before.scopes);
  assert.equal((await invoke('content_status')).indexed, 0);
  assert.deepEqual(requests, []);
  assert.deepEqual(instance.errors, []);
  const latest = await invoke('collection_get', { id: collection.id });
  await invoke('collection_remove', {
    kind: 'collection',
    id: collection.id,
    expected: latest.revision,
  });
  assert.equal(digest(await readFile(path)), digest(original));
  console.log(
    JSON.stringify({
      directory,
      source: path,
      spineEntries: pages.map((page) => page.location.entry),
      originalFirstPagePixel: pixel,
      textPixels,
      svgPixel,
      firstScreenshot,
      secondScreenshot,
      persistentCache: true,
      bookmark: true,
      sourceChangeRejected: true,
      originalPreserved: true,
      permissionsUnchanged: true,
      trackerRequests: requests.length,
      expectedSandboxDenials: policyDenials.length,
      errors: 0,
    }),
  );
} catch (error) {
  if (instance) {
    await instance.main.screenshot({ path: join(directory, 'failure.png') }).catch(() => {});
    console.log(JSON.stringify({ directory, errors: instance.errors }));
  }
  throw error;
} finally {
  if (instance) await shutdown(instance);
  await new Promise((done) => tracker.close(done));
}
