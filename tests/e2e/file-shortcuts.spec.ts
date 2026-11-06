// Browser plugin not available: repository Playwright. Native file actions are tested separately.
import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('keeps Enter for legacy settings and validates, cancels and persists a custom file opening key', async ({
  page,
}) => {
  await page.addInitScript(() => {
    if (!localStorage.getItem('prism.settings.v1'))
      localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 }));
  });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page.locator('.search-box kbd')).toHaveText('Enter');
  const open = async () => {
    await page.getByRole('button', { name: '설정', exact: true }).click();
    await page.getByRole('button', { name: '단축키', exact: true }).click();
  };
  const field = page.getByRole('textbox', { name: '파일·폴더 열기', exact: true });
  const save = page.getByRole('button', { name: '저장', exact: true });
  await open();
  await expect(field).toHaveValue('Enter');
  for (const invalid of [
    '',
    'o',
    'Shift+O',
    'Ctrl+Shift+Enter',
    'Alt+F4',
    'Ctrl++O',
    'Alt+Space',
  ]) {
    await field.fill(invalid);
    await expect(field).toHaveAttribute('aria-invalid', 'true');
    await expect(save).toBeDisabled();
    await expect(page.locator('#file-shortcut-error')).not.toBeEmpty();
  }
  await field.fill('Ctrl+O');
  await expect(save).toBeEnabled();
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await open();
  await expect(field).toHaveValue('Enter');
  await field.fill('Ctrl+O');
  await save.click();
  await expect(page.locator('.search-box kbd')).toHaveText('Ctrl+O');
  await page.reload();
  await expect(page.locator('.search-box kbd')).toHaveText('Ctrl+O');
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('=1+1');
  await expect(page.locator('.search-box kbd')).toHaveText('Enter');
  await input.fill('한국어');
  await expect(page.locator('.search-box kbd')).toHaveText('Ctrl+O');
  await open();
  await expect(field).toHaveValue('Ctrl+O');
  await page.getByLabel('단축키', { exact: true }).fill('Control+KeyO');
  await expect(page.locator('#file-shortcut-error')).toHaveText(
    '빠른 검색창 전역 단축키와 다른 키를 선택하세요.',
  );
  await expect(save).toBeDisabled();
  await page.getByLabel('단축키', { exact: true }).fill('Alt+Space');
  await page.setViewportSize({ width: 390, height: 844 });
  await field.scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await mkdir(join(tmpdir(), 'prism-ui-evidence'), { recursive: true });
  await page.screenshot({
    path: join(tmpdir(), 'prism-ui-evidence', 'file-shortcut-mobile.png'),
    animations: 'disabled',
  });
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await page.goto('/#quick');
  const quick = page.getByRole('textbox', { name: '검색어', exact: true });
  await quick.fill('한국어');
  await expect(page.locator('.quick-footer')).toContainText('Ctrl+O');
  await quick.fill('=1+1');
  await expect(page.locator('.quick-footer')).toContainText('Enter');
  expect(errors).toEqual([]);
});
