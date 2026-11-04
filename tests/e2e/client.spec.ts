import { expect, test } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const evidence = join(tmpdir(), 'prism-ui-evidence');
test.beforeAll(async () => {
  await mkdir(evidence, { recursive: true });
});
test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  (page as unknown as { appErrors: string[] }).appErrors = errors;
});
test.afterEach(async ({ page }) => {
  expect((page as unknown as { appErrors: string[] }).appErrors).toEqual([]);
});

test('renders the empty library without fabricated files', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page.getByRole('heading', { name: '검색할 폴더를 추가하세요' })).toBeVisible();
  await expect(page.getByText('브라우저 미리보기', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '본문', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: '폴더 추가', exact: true }).last().click();
  await expect(page.getByRole('status')).toContainText('데스크톱 앱');
  await page.getByRole('status').getByRole('button').click();
  await page.screenshot({ path: join(evidence, 'main.png') });
});

test('transfers query and filters to quick search and handles IME Enter', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('개발 계획');
  await page.getByRole('button', { name: '필터', exact: true }).click();
  await page.getByLabel('확장자', { exact: true }).fill('hwpx');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.getByRole('main', { name: '빠른 검색' })).toBeVisible();
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await expect(input).toHaveValue('개발 계획');
  await input.dispatchEvent('keydown', {
    key: 'Enter',
    code: 'Enter',
    isComposing: true,
    keyCode: 229,
  });
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.session.v1') ?? '{}').recentQueries ?? [],
      ),
    )
    .toEqual([]);
  await input.press('Enter');
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.session.v1') ?? '{}').recentQueries,
      ),
    )
    .toEqual(['개발 계획']);
  await input.press('Control+Enter');
  await expect(page.getByRole('heading', { name: '파일 검색', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '필터', exact: true }).click();
  await expect(page.getByLabel('확장자', { exact: true })).toHaveValue('hwpx');
  await page.getByRole('button', { name: '최근 검색', exact: true }).click();
  await expect(page.getByRole('button', { name: '개발 계획 ↗' })).toBeVisible();
});

test('persists theme, language and text size; cancellation does not apply changes', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('테마', { exact: true }).selectOption('dark');
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('테마', { exact: true }).selectOption('dark');
  await page.getByLabel('언어', { exact: true }).selectOption('en');
  await page.getByLabel('글자 크기', { exact: true }).fill('16');
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'File search', exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('html')).toHaveCSS('font-size', '16px');
  await page.screenshot({ path: join(evidence, 'dark.png') });
});

test('keeps filename and AI scopes independent', async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem(
      'prism.settings.v1',
      JSON.stringify({
        schemaVersion: 1,
        revision: 0,
        scopes: [
          {
            id: 'test-scope',
            name: '테스트 문서',
            path: 'C:\\PrismTest',
            filenameEnabled: true,
            contentEnabled: false,
          },
        ],
      }),
    ),
  );
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByRole('button', { name: '검색 범위', exact: true }).click();
  await page.getByLabel('본문·AI 색인', { exact: true }).check();
  await expect(page.getByLabel('파일명 검색', { exact: true })).toBeChecked();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').scopes[0].contentEnabled,
      ),
    )
    .toBe(true);
  await expect(page.getByRole('heading', { name: '검색 범위가 준비됐어요' })).toBeVisible();
  await expect(page.getByText('폴더 설정은 저장됐어요.', { exact: false })).toBeVisible();
});

test('edits, persists and exports a real Markdown draft', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '문서', exact: true }).click();
  await page.getByRole('textbox', { name: '문서 제목', exact: true }).fill('Prism 구현 메모');
  await page
    .getByRole('textbox', { name: 'Markdown', exact: true })
    .fill('# 첫 단계\n\n- 두 창\n- 설정 저장');
  await page.getByRole('button', { name: '채팅', exact: true }).click();
  await expect(page.getByRole('heading', { name: '내 문서를 근거로 대화하세요' })).toBeVisible();
  await page.getByRole('button', { name: '문서', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Markdown', exact: true })).toContainText(
    '- 설정 저장',
  );
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Markdown 저장', exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('Prism 구현 메모.md');
  expect(await readFile((await download.path())!, 'utf8')).toContain('# 첫 단계');
  await page.screenshot({ path: join(evidence, 'documents.png') });
});

test('does not retain search history when disabled', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('이전 검색 상태 유지', { exact: true }).uncheck();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('임시 검색');
  await page.getByRole('textbox', { name: '검색어', exact: true }).press('Enter');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.getByRole('textbox', { name: '검색어', exact: true })).toHaveValue('');
  await page.reload();
  expect(await page.evaluate(() => localStorage.getItem('prism.session.v1'))).toBeNull();
});

test('shows responsive settings and quick search without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('button', { name: '한국어', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '설정', exact: true })).toBeVisible();
  await page.screenshot({ path: join(evidence, 'settings-mobile.png') });
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await page.screenshot({ path: join(evidence, 'mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('   ');
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('검색');
  await expect(page.getByRole('heading', { name: '검색할 폴더가 없어요' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('textbox', { name: '검색어', exact: true }).press('Escape');
  await expect(page.getByRole('heading', { name: '파일 검색', exact: true })).toBeVisible();
});

test('captures desktop reference states', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 1190, height: 846 },
    deviceScaleFactor: 1.25,
    colorScheme: 'light',
  });
  const page = await context.newPage();
  await page.goto('http://127.0.0.1:1420');
  await expect(page.getByRole('heading', { name: '검색할 폴더를 추가하세요' })).toBeVisible();
  await page.screenshot({ path: join(evidence, 'main-reference.png') });
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.screenshot({ path: join(evidence, 'settings.png') });
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.screenshot({ path: join(evidence, 'quick.png') });
  await context.close();
});
