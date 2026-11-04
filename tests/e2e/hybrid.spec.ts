import { test, expect } from '@playwright/test';
import { tmpdir } from 'node:os';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';

test('combines natural-language and keyword results by default and saves the opt-out explicitly', async ({
  page,
}) => {
  await mkdir(join(tmpdir(), 'prism-ui-evidence'), { recursive: true });
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
  const open = async () => {
    await page.getByRole('button', { name: '설정', exact: true }).click();
    await page.getByRole('button', { name: 'AI·모델', exact: true }).click();
  };
  const toggle = page.getByLabel('자연어 검색에 본문 키워드 결과 결합', { exact: true });
  await open();
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
        () => JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').hybridSearch,
      ),
    )
    .toBe(false);
  await page.reload();
  await open();
  await expect(toggle).not.toBeChecked();
  await page.setViewportSize({ width: 390, height: 844 });
  await toggle.scrollIntoViewIfNeeded();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({
    path: join(tmpdir(), 'prism-ui-evidence', 'hybrid-settings-mobile.png'),
    animations: 'disabled',
  });
  await toggle.check();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').hybridSearch,
      ),
    )
    .toBe(true);
  expect(errors).toEqual([]);
});
