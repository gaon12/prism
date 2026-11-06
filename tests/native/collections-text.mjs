// Browser plugin not available: actual bundled WebView, source extraction,
// browser text rendering, SQLite and native WebP admission; no media IPC mocks.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';
import { launch, shutdown, until } from './harness.mjs';

assert(process.env.PRISM_TEST_EXECUTABLE, 'Select the candidate executable explicitly');
const release = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(release));
const root = await mkdtemp(join(tmpdir(), 'prism-text-pages-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE);
const executable = join(portable, 'prism.exe');
await copyFile(candidate, executable);
writePackageNotices(portable, loadPackageNotices(candidate));
await writeFile(join(portable, 'prism.portable'), '');
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const executableSha256 = digest(await readFile(executable));
assert.equal(digest(await readFile(candidate)), executableSha256);
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    scopes: [],
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F10',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
  }),
);
const source = join(root, '소설 첫 페이지.txt');
const original =
  '제1장 — 첫 페이지의 한국어 日本語 中文 😀\n' +
  '<script>window.BAD_TEXT_SCRIPT = true</script>\n' +
  '<img src="https://invalid.example/never-load"> & literal text\n' +
  '첫 번째 페이지의 원문입니다. 읽기 화면과 같은 글자가 보여야 합니다.\n'.repeat(120);
await writeFile(source, original);
const officeSource = join(root, 'Office 원문.docx');
const officeOriginal = await readFile('tests/fixtures/encrypted-office/example.docx');
await writeFile(officeSource, officeOriginal);
let instance;
let collection, work, unit, pages, firstCover;
const errors = [];
let expectedSandboxDenials = 0;
const collectErrors = (running) => {
  for (const error of running.errors) {
    // The native document-start initialization is denied in unscripted
    // srcdoc frames, exactly as in the existing EPUB reader scenario.
    if (
      error ===
      "Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set."
    )
      expectedSandboxDenials++;
    else errors.push(error);
  }
};
const network = [];
const settingsBefore = JSON.parse(await readFile(join(data, 'settings.json'), 'utf8'));
// Serialize native error objects before crossing Playwright's evaluation
// boundary; otherwise an expected IPC rejection is only reported as "Object".
const checkedInvoke =
  (page) =>
  async (command, args = {}) => {
    const result = await page.evaluate(
      async ({ command, args }) => {
        try {
          return { value: await window.__TAURI_INTERNALS__.invoke(command, args) };
        } catch (error) {
          return { error };
        }
      },
      { command, args },
    );
    if (result.error) throw Object.assign(new Error(JSON.stringify(result.error)), result.error);
    return result.value;
  };
try {
  instance = await launch(data, { executable, portable: true });
  let { main, invoke } = instance;
  invoke = checkedInvoke(main);
  main.on('request', (request) => {
    if (request.url().startsWith('https://invalid.example/')) network.push(request.url());
  });
  assert.equal((await invoke('runtime_info')).development, release === '0');
  const before = await invoke('get_settings');
  collection = await invoke('collection_create', { title: '텍스트 책장', description: '' });
  work = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: '첫 페이지를 읽는 소설',
    metadata: {
      kind: 'novel',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  unit = await invoke('collection_unit_create', {
    workId: work.id,
    title: '제1권',
    kind: 'volume',
  });
  unit = await invoke('collection_import', {
    unitId: unit.id,
    expected: unit.revision,
    paths: [source],
  });
  pages = (
    await invoke('collection_pages', {
      unitId: unit.id,
      expected: unit.revision,
      offset: 0,
      limit: 48,
    })
  ).items;
  assert(pages.length > 1);
  const uncached = await invoke('collection_cover', { workId: work.id });
  assert.equal(uncached.pageId, pages[0].id);
  assert.equal(uncached.sourceHash, digest(Buffer.from(original)));
  assert(uncached.text.startsWith('제1장') && !uncached.dataUrl);
  const officeWork = await invoke('collection_work_create', {
    collectionId: collection.id,
    title: 'Office 텍스트 미리보기',
    metadata: {
      kind: 'document',
      direction: 'leftToRight',
      authors: [],
      tags: [],
      description: '',
      externalId: null,
    },
  });
  const officeUnit = await invoke('collection_unit_create', {
    workId: officeWork.id,
    title: '문서',
    kind: 'document',
  });
  await invoke('collection_import', {
    unitId: officeUnit.id,
    expected: officeUnit.revision,
    paths: [officeSource],
  });
  const officePreview = await invoke('collection_cover', { workId: officeWork.id });
  assert(officePreview.text && !officePreview.document, 'Office stays an extracted-text preview');
  await main.getByRole('button', { name: '컬렉션', exact: true }).click();
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  const card = region.getByRole('button', { name: work.title, exact: true });
  await expect(card.locator('img')).toBeVisible({ timeout: 25_000 });
  await expect(
    region.getByRole('button', { name: officeWork.title, exact: true }).locator('img'),
  ).toBeVisible({ timeout: 25_000 });
  assert(
    (await invoke('collection_cover', { workId: officeWork.id })).dataUrl.startsWith(
      'data:image/webp;base64,',
    ),
  );
  firstCover = await invoke('collection_cover', { workId: work.id });
  assert(firstCover.dataUrl.startsWith('data:image/webp;base64,') && firstCover.text === null);
  const pixels = await main.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const data = context.getImageData(0, 0, image.width, image.height).data;
    let ink = 0;
    for (let n = 0; n < data.length; n += 4)
      if (Math.min(data[n], data[n + 1], data[n + 2]) < 180) ink++;
    return { width: image.width, height: image.height, ink };
  }, firstCover.dataUrl);
  assert.equal(pixels.height, 480);
  assert(
    pixels.width > 300 && pixels.width < 400 && pixels.ink > 1000,
    'actual text ink, not an empty or generic cover',
  );
  await main.screenshot({ path: join(root, 'text-gallery.png') });
  await card.click();
  const pageButton = region.locator('.collection-page-card > button').first();
  await until(
    async () => (await region.locator('.collection-page-card').count()) > 1,
    'text pages listed',
  );
  await pageButton.click();
  const reader = main.getByRole('dialog', { name: work.title, exact: true });
  await expect(reader.getByText('텍스트 미리보기', { exact: true })).toBeVisible();
  const frame = reader.locator('iframe');
  await expect(frame).toBeVisible();
  await until(
    () =>
      frame.evaluate((node) =>
        node.contentDocument?.querySelector('pre')?.textContent?.startsWith('제1장'),
      ),
    'actual text reader',
  );
  const plain = await invoke('collection_media', { pageId: pages[0].id, thumbnail: false });
  const rendered = await frame.evaluate((node) => ({
    text: node.contentDocument.querySelector('pre').textContent,
    scripts: node.contentDocument.querySelectorAll('script,img').length,
    leaked: node.contentWindow.BAD_TEXT_SCRIPT === true,
    font: node.contentWindow.getComputedStyle(node.contentDocument.querySelector('pre')).font,
  }));
  assert.equal(rendered.text, plain.text);
  assert.equal(rendered.scripts, 0);
  assert.equal(rendered.leaked, false);
  await main.frameLocator('.collection-reader iframe').locator('pre').click();
  await frame.evaluate((node) => node.contentWindow.scrollTo({ top: 0, behavior: 'instant' }));
  await until(
    () => frame.evaluate((node) => node.contentWindow.scrollY === 0),
    'keyboard test starts at page top',
  );
  await main.keyboard.press('PageDown');
  await until(
    () => frame.evaluate((node) => node.contentWindow.scrollY > 0),
    'PageDown scrolls inside the current text page',
  );
  let previousScroll = -1;
  let stableScroll = 0;
  await until(async () => {
    const current = await frame.evaluate((node) => node.contentWindow.scrollY);
    stableScroll = current === previousScroll ? stableScroll + 1 : 0;
    previousScroll = current;
    return stableScroll >= 3;
  }, 'browser PageDown scroll animation settles');
  await expect(reader.getByText(`1 / ${pages.length} 페이지`, { exact: true })).toBeVisible();
  await frame.evaluate((node) => node.contentWindow.scrollTo({ top: 0, behavior: 'instant' }));
  await until(
    () => frame.evaluate((node) => node.contentWindow.scrollY === 0),
    'reader at page start',
  );
  await main.screenshot({ path: join(root, 'text-reader.png') });
  const narrow = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...narrow, theme: 'dark' },
    expectedRevision: narrow.revision,
  });
  await main.setViewportSize({ width: 390, height: 760 });
  await until(
    () => frame.evaluate((node) => node.contentWindow.innerWidth < 390),
    'narrow text reflow',
  );
  assert(
    await frame.evaluate(
      (node) =>
        node.contentDocument.documentElement.scrollWidth <= node.contentWindow.innerWidth + 1,
    ),
  );
  assert(
    await reader
      .locator('.collection-reader-page')
      .evaluate((element) => element.scrollHeight <= element.clientHeight + 1),
    'text has one scrolling viewport',
  );
  await frame.evaluate((node) => node.contentWindow.scrollTo({ top: 0, behavior: 'instant' }));
  await until(
    () => frame.evaluate((node) => node.contentWindow.scrollY === 0),
    'narrow reader at page start',
  );
  await main.screenshot({ path: join(root, 'text-reader-narrow.png') });
  await main.frameLocator('.collection-reader iframe').locator('pre').click();
  // Focus first: Playwright's click scrolls a long <pre> into view. Establish
  // the bottom boundary only afterwards so PageDown exercises that boundary.
  await frame.evaluate((node) =>
    node.contentWindow.scrollTo({
      top: node.contentDocument.documentElement.scrollHeight,
      behavior: 'instant',
    }),
  );
  await until(
    () =>
      frame.evaluate(
        (node) =>
          node.contentDocument.documentElement.scrollHeight -
            node.contentWindow.innerHeight -
            node.contentWindow.scrollY <=
          1,
      ),
    'actual text-page bottom before navigation',
  );
  await main.keyboard.press('PageDown');
  await expect(reader.getByText(`2 / ${pages.length} 페이지`, { exact: true })).toBeVisible();
  await until(
    async () => (await invoke('collection_reading', { workId: work.id })).pageId === pages[1].id,
    'reading bookmark follows the next actual text page',
  );
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  await main.setViewportSize({ width: 1600, height: 1050 });
  const second = pages[1];
  // Moving an actual page to the beginning changes the work's cover. It must
  // never keep a file-name card or a thumbnail attached to the old first page.
  unit = await invoke('collection_move_page', {
    unitId: unit.id,
    expected: unit.revision,
    pageId: second.id,
    before: pages[0].id,
  });
  const reordered = await invoke('collection_cover', { workId: work.id });
  assert.equal(reordered.pageId, second.id);
  const secondText = await invoke('collection_media', { pageId: second.id, thumbnail: false });
  assert(secondText.text && !secondText.text.startsWith('제1장'));
  assert(
    reordered.text || reordered.dataUrl,
    'the new first page may already have a cached viewport',
  );
  await invoke('collection_move_page', {
    unitId: unit.id,
    expected: unit.revision,
    pageId: pages[0].id,
    before: second.id,
  });
  const after = await invoke('get_settings');
  assert.deepEqual(after.scopes, before.scopes);
  assert.equal(after.generation.allowExternalQuestions, before.generation.allowExternalQuestions);
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  collectErrors(instance);
  await shutdown(instance);
  instance = null;
  instance = await launch(data, { executable, portable: true });
  ({ main, invoke } = instance);
  invoke = checkedInvoke(main);
  const restored = await invoke('collection_cover', { workId: work.id });
  assert.equal(restored.dataUrl, firstCover.dataUrl, 'actual cached viewport survives restart');
  // A native cache write reauthenticates original bytes as well as the page
  // and fingerprint supplied by the caller. Cached reads must do so too.
  assert.equal(await readFile(source, 'utf8'), original);
  const changedOriginal = '변경된 첫 페이지 원문입니다.\n'.repeat(130);
  await writeFile(source, changedOriginal);
  await assert.rejects(
    invoke('collection_cover', { workId: work.id }),
    (error) => error.code === 'sourceChanged',
  );
  await assert.rejects(
    invoke('collection_cache_text', {
      pageId: pages[0].id,
      sourceHash: pages[0].sourceHash,
      dataUrl: firstCover.dataUrl,
    }),
    (error) => error.code === 'sourceChanged',
  );
  const currentUnit = await invoke('collection_unit_get', { id: unit.id });
  unit = await invoke('collection_import', {
    unitId: unit.id,
    expected: currentUnit.revision,
    paths: [],
  });
  const refreshed = await invoke('collection_cover', { workId: work.id });
  assert(refreshed.text.startsWith('변경된') && !refreshed.dataUrl);
  assert.notEqual(refreshed.sourceHash, firstCover.sourceHash);
  assert.equal(await readFile(source, 'utf8'), changedOriginal);
  assert.equal((await invoke('get_settings')).clipboard.enabled, settingsBefore.clipboard.enabled);
  assert.equal((await invoke('get_settings')).scopes.length, 0);
  collectErrors(instance);
  assert.deepEqual(errors, []);
  assert.deepEqual(network, []);
  assert.equal(digest(await readFile(candidate)), executableSha256);
  assert.equal(digest(await readFile(officeSource)), digest(officeOriginal));
  const result = {
    passed: true,
    root,
    executableSha256,
    release: release === '1',
    textPages: pages.length,
    actualInkPixels: pixels.ink,
    readerFont: rendered.font,
    expectedSandboxDenials,
    reorderedCover: true,
    restartedCache: true,
    changedSourceRejected: true,
    literalMarkupOnly: true,
    officeExtractedTextThumbnail: true,
    contentScopesEnabled: 0,
    externalAIRequests: 0,
    errors: 0,
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} catch (error) {
  console.log(JSON.stringify({ root, failed: true, errors: instance?.errors }));
  if (instance) await instance.main.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
