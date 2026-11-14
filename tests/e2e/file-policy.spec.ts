import { expect, test, type Locator, type Page } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const evidence = process.env.PRISM_UI_EVIDENCE ?? join(tmpdir(), 'prism-file-policy-ui');
test.beforeAll(() => mkdir(evidence, { recursive: true }));

// Exact browser/hot-refresh compatibility notices, retained as evidence.
// Every page error and every other console warning/error still fails.
const compatibilityNotices = new Set([
  "Error with Permissions-Policy header: Unrecognized feature: 'join-ad-interest-group'.",
  "Error with Permissions-Policy header: Unrecognized feature: 'run-ad-auction'.",
  'Something has shimmed the React DevTools global hook (__REACT_DEVTOOLS_GLOBAL_HOOK__). Fast Refresh is not compatible with this shim and will be disabled.',
]);
const browserNotices = new WeakMap<Page, object[]>();

async function settleDialog(dialog: Locator) {
  // Let finite modal/switch transitions finish naturally before capturing paint.
  await dialog.evaluate(async (element) => {
    await Promise.all(
      element
        .getAnimations({ subtree: true })
        .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
        .map((animation) => animation.finished.catch(() => {})),
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });
}
test.afterEach(async ({ page, browser }, info) => {
  await info.attach('browser-compatibility-notices', {
    body: JSON.stringify(
      { browser: browser.version(), url: page.url(), notices: browserNotices.get(page) ?? [] },
      null,
      2,
    ),
    contentType: 'application/json',
  });
});

async function prepare(page: Page, language = 'ko') {
  const messages: string[] = [];
  const notices: object[] = [];
  browserNotices.set(page, notices);
  page.on('pageerror', (error) => messages.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error' || message.type() === 'warning') {
      if (compatibilityNotices.has(message.text())) {
        notices.push({ level: message.type(), text: message.text(), location: message.location() });
      } else messages.push(message.text());
    }
  });
  await page.addInitScript((language) => {
    if (!localStorage.getItem('prism.settings.v1'))
      localStorage.setItem(
        'prism.settings.v1',
        JSON.stringify({
          onboardingVersion: 1,
          language,
          scopes: [
            {
              id: 'owned',
              name: 'Owned scope',
              path: 'C:\\fixtures\\owned',
              filenameEnabled: true,
              contentEnabled: false,
              externalAllowed: false,
            },
          ],
        }),
      );
  }, language);
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page).toHaveURL(/http:\/\/127\.0\.0\.1:142[01]\//);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  return messages;
}

test('saves independent inclusion rules, preserves folder permissions and discards cancelled edits', async ({
  page,
}) => {
  const errors = await prepare(page);
  const open = async () => {
    await page.getByRole('button', { name: '설정', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '설정', exact: true });
    await dialog.getByRole('button', { name: '검색 범위', exact: true }).click();
    return dialog;
  };
  let dialog = await open();
  const policy = () => dialog.locator('.file-policy-settings');
  const filenames = () => policy().getByRole('group', { name: '파일명 검색', exact: true });
  const content = () => policy().getByRole('group', { name: '본문·AI 색인', exact: true });
  await expect(policy().getByRole('checkbox')).toHaveCount(10);
  for (const checkbox of await policy().getByRole('checkbox').all())
    await expect(checkbox).toBeChecked();
  for (const checkbox of await filenames().getByRole('checkbox').all()) await checkbox.uncheck();
  await content().getByRole('checkbox', { name: '소스 코드', exact: true }).uncheck();
  await expect(content().getByRole('checkbox', { checked: true })).toHaveCount(4);
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const stored = await page.evaluate(() => JSON.parse(localStorage.getItem('prism.settings.v1')!));
  expect(stored.filePolicy).toEqual({
    filename: {
      hidden: false,
      system: false,
      sourceCode: false,
      repositoryIgnored: false,
      archives: false,
    },
    content: {
      hidden: true,
      system: true,
      sourceCode: false,
      repositoryIgnored: true,
      archives: true,
    },
  });
  expect(stored.scopes).toHaveLength(1);
  expect(stored.scopes[0]).toMatchObject({
    filenameEnabled: true,
    contentEnabled: false,
    externalAllowed: false,
  });
  await page.reload();
  dialog = await open();
  await expect(filenames().getByRole('checkbox', { checked: true })).toHaveCount(0);
  await expect(content().getByRole('checkbox', { checked: true })).toHaveCount(4);
  await policy().scrollIntoViewIfNeeded();
  await settleDialog(dialog);
  await page.screenshot({
    path: join(evidence, 'inclusion-desktop-saved.png'),
  });
  await filenames().getByRole('checkbox', { name: '숨김 항목', exact: true }).check();
  await dialog.getByRole('button', { name: '취소', exact: true }).click();
  dialog = await open();
  await expect(
    filenames().getByRole('checkbox', { name: '숨김 항목', exact: true }),
  ).not.toBeChecked();
  expect(errors).toEqual([]);
});

for (const [language, settings, scopes, title, filename, content, archive, save] of [
  ['ko', '설정', '검색 범위', '포함할 항목', '파일명 검색', '본문·AI 색인', '압축 파일', '저장'],
  [
    'en',
    'Settings',
    'Search scopes',
    'Items to include',
    'Filename search',
    'Content · AI indexing',
    'Archives',
    'Save',
  ],
  ['zh-CN', '设置', '搜索范围', '包含项目', '文件名搜索', '正文 · AI 索引', '压缩文件', '保存'],
  ['zh-TW', '設定', '搜尋範圍', '包含項目', '檔名搜尋', '內文 · AI 索引', '壓縮檔', '儲存'],
  [
    'ja',
    '設定',
    '検索範囲',
    '含める項目',
    'ファイル名検索',
    '本文・AI索引',
    '圧縮ファイル',
    '保存',
  ],
]) {
  for (const [width, height] of [
    [390, 844],
    [760, 540],
  ] as const) {
    const size = width === 390 ? 'a narrow screen' : 'the minimum window';
    test(`edits localized inclusion rules on ${size}: ${language}`, async ({ page }, info) => {
      await page.setViewportSize({ width, height });
      const errors = await prepare(page, language);
      await page.getByRole('button', { name: settings, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: settings, exact: true });
      await dialog.getByRole('button', { name: scopes, exact: true }).click();
      const policy = dialog.locator('.file-policy-settings');
      await expect(policy.getByRole('heading', { name: title, exact: true })).toBeVisible();
      const filenames = policy.getByRole('group', { name: filename, exact: true });
      const body = policy.getByRole('group', { name: content, exact: true });
      await body.getByRole('checkbox', { name: archive, exact: true }).uncheck();
      await expect(filenames.getByRole('checkbox', { name: archive, exact: true })).toBeChecked();
      await body.scrollIntoViewIfNeeded();
      await settleDialog(dialog);
      const controls = await policy.evaluate((section) => {
        const pane = section.closest('.settings-body') as HTMLElement;
        const clip = pane.getBoundingClientRect();
        return [...section.querySelectorAll('input')].map((input) => {
          const rect = input.getBoundingClientRect();
          return {
            label: input.closest('label')?.textContent?.trim(),
            left: rect.left,
            right: rect.right,
            top: rect.top,
            bottom: rect.bottom,
            checked: input.checked,
            clipLeft: clip.left,
            clipRight: clip.left + pane.clientWidth,
            clipTop: clip.top,
            clipBottom: clip.top + pane.clientHeight,
            scrollLeft: pane.scrollLeft,
            width: rect.width,
            background: getComputedStyle(input).backgroundColor,
          };
        });
      });
      for (const control of controls) {
        expect(control.left, JSON.stringify(control)).toBeGreaterThanOrEqual(control.clipLeft);
        expect(control.right, JSON.stringify(control)).toBeLessThanOrEqual(control.clipRight);
        expect(control.width, JSON.stringify(control)).toBeGreaterThanOrEqual(24);
      }
      for (const control of controls.slice(5)) {
        expect(control.top, JSON.stringify(control)).toBeGreaterThanOrEqual(control.clipTop);
        expect(control.bottom, JSON.stringify(control)).toBeLessThanOrEqual(control.clipBottom);
      }
      await info.attach('switch-layout', {
        body: JSON.stringify(controls, null, 2),
        contentType: 'application/json',
      });
      await page.screenshot({
        path: join(evidence, `inclusion-${language}-${width}.png`),
      });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      const box = await dialog.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(0);
      expect(box!.width).toBeLessThanOrEqual(width);
      for (const button of await dialog.locator('.modal-footer button').all()) {
        const bounds = await button.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.y).toBeGreaterThanOrEqual(0);
        expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(height);
      }
      await dialog.getByRole('button', { name: save, exact: true }).click();
      await expect(dialog).toHaveCount(0);
      expect(
        await page.evaluate(
          () => JSON.parse(localStorage.getItem('prism.settings.v1')!).filePolicy,
        ),
      ).toMatchObject({ filename: { archives: true }, content: { archives: false } });
      expect(errors).toEqual([]);
    });
  }
}
