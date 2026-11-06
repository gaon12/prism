// Real EPUB/SQLite/navigation/reader IPC. Browser plugin absent: repository
// Playwright/WebView2. No media, document, navigation or model responses mocked.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';
import { launch, shutdown, until } from './harness.mjs';

assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply an explicit candidate executable');
const release = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(release));
const root = await mkdtemp(join(tmpdir(), 'prism-epub-navigation-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE);
const executable = join(portable, 'prism.exe');
await copyFile(candidate, executable);
writePackageNotices(portable, loadPackageNotices(candidate));
await writeFile(join(portable, 'prism.portable'), '');
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const executableSha256 = hash(await readFile(executable));
assert.equal(hash(await readFile(candidate)), executableSha256);
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
const books = [];
for (const [fixture, title] of [
  ['navigation', '목차 소설'],
  ['navigation-ncx', 'NCX 소설'],
  ['navigation-spine', '장 목록 소설'],
  ['navigation-many', '긴 목차 소설'],
]) {
  const original = await readFile(`tests/fixtures/epub/${fixture}.epub`);
  const path = join(root, `${title}.epub`);
  await writeFile(path, original);
  books.push({ fixture, title, original, path });
}
const errors = [],
  network = [];
let expectedSandboxDenials = 0,
  instance;
const collectErrors = () => {
  for (const error of instance.errors) {
    if (
      error ===
      "Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set."
    )
      expectedSandboxDenials++;
    else errors.push(error);
  }
};
const checkedInvoke =
  (main) =>
  async (command, args = {}) => {
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
    if (result.error) throw Object.assign(new Error(JSON.stringify(result.error)), result.error);
    return result.value;
  };
try {
  instance = await launch(data, { executable, portable: true });
  const { main } = instance;
  const invoke = checkedInvoke(main);
  main.on('request', (request) => {
    if (
      /^https?:/.test(request.url()) &&
      !['ipc.localhost', 'tauri.localhost'].includes(new URL(request.url()).hostname)
    )
      network.push(request.url());
  });
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.installKind, 'portable');
  assert.equal(runtime.development, release === '0');
  const before = await invoke('get_settings');
  const collection = await invoke('collection_create', { title: '목차 책장', description: '' });
  for (const book of books) {
    book.work = await invoke('collection_work_create', {
      collectionId: collection.id,
      title: book.title,
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
      workId: book.work.id,
      title: '제1권',
      kind: 'volume',
    });
    book.unit = await invoke('collection_import', {
      unitId: unit.id,
      expected: unit.revision,
      paths: [book.path],
    });
    book.pages = (
      await invoke('collection_pages', {
        unitId: unit.id,
        expected: book.unit.revision,
        offset: 0,
        limit: 10,
      })
    ).items;
    assert.equal(book.pages.length, 2);
  }
  const book = books[0];
  const navigation = await invoke('collection_navigation', {
    pageId: book.pages[0].id,
    expected: book.unit.revision,
  });
  assert.equal(navigation.items[1].title, '1 시작 뒤');
  assert.equal(navigation.items[2].fragment, '후반:위치');
  assert(navigation.items.slice(5).every((item) => item.pageId === null));
  const region = main.getByRole('region', { name: '컬렉션', exact: true });
  const reader = main.locator('dialog.collection-reader');
  const frame = reader.locator('iframe');
  const openBook = async (item) => {
    await main.reload();
    await main.getByRole('button', { name: '컬렉션', exact: true }).click();
    const card = region.getByRole('button', { name: item.title, exact: true });
    await expect(card).toBeVisible();
    await card.click();
    await region.locator('.collection-page-card > button').first().click();
    await expect(frame).toBeVisible();
  };
  const contents = main.getByRole('complementary', { name: '목차', exact: true });
  const openContents = async () => {
    await reader.getByRole('button', { name: '목차', exact: true }).click();
    await expect(contents).toBeVisible();
  };
  const jump = async (label) => {
    await openContents();
    await contents.getByRole('button', { name: label, exact: true }).click();
    await expect(contents).not.toBeVisible();
  };
  const anchorPosition = () =>
    frame.evaluate((node) => {
      const target = node.contentDocument?.getElementById('후반:위치');
      return target
        ? { top: target.getBoundingClientRect().top, scrollY: node.contentWindow.scrollY }
        : null;
    });
  await openBook(book);
  await jump('후반으로 이동');
  await until(async () => {
    const p = await anchorPosition();
    return p && Math.abs(p.top) < 40 && p.scrollY > 1000;
  }, 'actual Korean fragment is scrolled into the viewport');
  await until(
    async () =>
      (await invoke('collection_reading', { workId: book.work.id })).pageId === book.pages[1].id,
    'chapter jump saves the actual reading page',
  );
  const firstJump = await anchorPosition();
  await main.screenshot({ path: join(root, 'fragment-destination.png') });
  await openContents();
  await contents.getByRole('textbox', { name: '목차에서 찾기' }).fill('장 처음');
  await expect(contents.locator('li button')).toHaveCount(1);
  await contents.getByRole('button', { name: '장 처음', exact: true }).click();
  await until(
    () => frame.evaluate((node) => node.contentWindow.scrollY === 0),
    'same chapter can jump back to the beginning',
  );
  await jump('없는 위치');
  await expect(reader.getByRole('status').filter({ hasText: '지정된 위치' })).toBeVisible();
  await openContents();
  assert.equal(
    await contents.getByRole('button', { name: '외부 이동 금지', exact: true }).count(),
    0,
  );
  await contents.getByRole('textbox', { name: '목차에서 찾기' }).fill('일치하지않음');
  await expect(contents.getByText('일치하는 목차가 없어요.', { exact: true })).toBeVisible();
  await main.keyboard.press('Escape');
  await expect(contents).not.toBeVisible();
  await expect(reader).toBeVisible();
  await expect(reader.getByRole('button', { name: '목차', exact: true })).toBeFocused();
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  const reordered = await invoke('collection_move_page', {
    unitId: book.unit.id,
    expected: book.unit.revision,
    pageId: book.pages[1].id,
    before: book.pages[0].id,
  });
  await assert.rejects(
    invoke('collection_navigation', { pageId: book.pages[0].id, expected: book.unit.revision }),
    (e) => e.code === 'conflict',
  );
  book.unit = reordered;
  const projected = await invoke('collection_navigation', {
    pageId: book.pages[0].id,
    expected: reordered.revision,
  });
  assert.equal(projected.items[1].offset, 1);
  assert.equal(projected.items[2].offset, 0);
  await openBook(book);
  await jump('1 시작 뒤');
  await until(
    () =>
      frame.evaluate(
        (node) =>
          node.contentDocument?.getElementById('시작')?.textContent === 'NAV FIRST CHAPTER 32917',
      ),
    'declared contents still finds first chapter after reader reorder',
  );
  await main.setViewportSize({ width: 390, height: 844 });
  await openContents();
  await expect(contents.getByRole('button', { name: '후반으로 이동', exact: true })).toBeVisible();
  const narrow = await reader.evaluate((node) => ({
    width: node.getBoundingClientRect().width,
    overflow: node.scrollWidth > node.clientWidth + 1,
  }));
  assert(narrow.width < 391 && !narrow.overflow);
  await main.screenshot({ path: join(root, 'contents-narrow.png') });
  await contents.getByRole('button', { name: '후반으로 이동', exact: true }).click();
  await until(async () => (await anchorPosition())?.scrollY > 1000, 'narrow fragment jump');
  await main.setViewportSize({ width: 1280, height: 900 });
  for (const [other, label] of [
    [books[1], 'NCX 후반'],
    [books[2], 'second.xhtml'],
  ]) {
    await reader.getByRole('button', { name: '닫기', exact: true }).click();
    await openBook(other);
    await jump(label);
    await until(
      () => frame.evaluate((node) => !!node.contentDocument?.getElementById('후반:위치')),
      'legacy/fallback chapter content is real',
    );
    if (other.fixture === 'navigation-ncx')
      await until(async () => (await anchorPosition())?.scrollY > 1000, 'NCX fragment scroll');
  }
  await reader.getByRole('button', { name: '닫기', exact: true }).click();
  await openBook(books[3]);
  await openContents();
  await expect(contents.locator('li button')).toHaveCount(100);
  await contents.getByRole('button', { name: '목차 더 보기', exact: true }).click();
  await expect(contents.locator('li button')).toHaveCount(200);
  await contents.getByRole('textbox', { name: '목차에서 찾기' }).fill('항목 0239');
  await expect(contents.locator('li button')).toHaveCount(1);
  await contents.getByRole('button', { name: '목차 항목 0239', exact: true }).click();
  await until(
    async () => (await anchorPosition())?.scrollY > 1000,
    'filtered item outside the initial render batch reaches the real fragment',
  );
  for (const item of books) assert.equal(hash(await readFile(item.path)), hash(item.original));
  const after = await invoke('get_settings');
  assert.deepEqual(after.scopes, before.scopes);
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  await writeFile(book.path, 'changed fixture source');
  await assert.rejects(
    invoke('collection_navigation', { pageId: book.pages[0].id, expected: book.unit.revision }),
    (e) => e.code === 'sourceChanged',
  );
  await writeFile(book.path, book.original);
  const source = (await invoke('collection_sources', { unitId: book.unit.id }))[0];
  await invoke('collection_remove_source', { id: source.id, expected: source.revision });
  await assert.rejects(
    invoke('collection_navigation', { pageId: book.pages[0].id, expected: book.unit.revision }),
    (e) => e.code === 'notFound',
  );
  collectErrors();
  assert.deepEqual(network, []);
  assert.deepEqual(errors, []);
  const result = {
    passed: true,
    root,
    executableSha256,
    release: release === '1',
    actualFragment: firstJump,
    declaredNav: true,
    legacyNcx: true,
    spineFallback: true,
    reorderedOffsets: true,
    narrow,
    originalFilesPreserved: true,
    changedSourceRejected: true,
    removedReferenceRejected: true,
    expectedSandboxDenials,
    errors: 0,
  };
  await writeFile(join(root, 'navigation-proof.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} catch (error) {
  if (instance) await instance.main.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
  console.log(JSON.stringify({ failed: true, root, error: String(error) }));
  throw error;
} finally {
  if (instance) await shutdown(instance);
}
