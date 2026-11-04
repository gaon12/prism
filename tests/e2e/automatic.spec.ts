// Browser plugin not available: exercise the repository's real production UI.
import { test, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('starts fresh sessions in automatic mode and preserves manual modes and auto preferences', async ({
  page,
}) => {
  await page.addInitScript(() => {
    if (!localStorage.getItem('prism.settings.v1'))
      localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 }));
  });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByRole('button', { name: '자동', exact: true })).toHaveClass(/active/);
  await page.getByRole('button', { name: '파일명', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => JSON.parse(localStorage.getItem('prism.session.v1') ?? '{}').mode),
    )
    .toBe('filename');
  await page.reload();
  await expect(page.getByRole('button', { name: '파일명', exact: true })).toHaveClass(/active/);
  await page.getByRole('button', { name: '자동', exact: true }).click();
  const open = async () => {
    await page.getByRole('button', { name: '설정', exact: true }).click();
    await page.getByRole('button', { name: 'AI·모델', exact: true }).click();
  };
  await open();
  const toggle = page.getByLabel('자동 모드에서 자연어 검색 사용', { exact: true });
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await open();
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').automaticSemantic,
      ),
    )
    .toBe(false);
  await page.reload();
  await open();
  await expect(toggle).not.toBeChecked();
  await page.getByRole('button', { name: '취소', exact: true }).click();
  expect(errors).toEqual([]);
});

test('wraps the English automatic mode toolbar at mobile width without fabricated results', async ({
  page,
}) => {
  await mkdir(join(tmpdir(), 'prism-ui-evidence'), { recursive: true });
  await page.addInitScript(() =>
    localStorage.setItem(
      'prism.settings.v1',
      JSON.stringify({ onboardingVersion: 1, language: 'en' }),
    ),
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Automatic', exact: true })).toHaveClass(/active/);
  for (const button of await page.locator('.search-modes button').all()) {
    const box = await button.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({
    path: join(tmpdir(), 'prism-ui-evidence', 'automatic-mobile.png'),
    animations: 'disabled',
  });
  await page.getByRole('button', { name: 'Quick search', exact: true }).click();
  const input = page.getByRole('textbox', { name: 'Search term', exact: true });
  await input.fill('   ');
  await expect(page.locator('.quick-results')).toHaveCount(0);
  await expect(page.locator('.quick-meta')).toHaveCount(0);
});
