// Browser plugin not available: existing production-preview Playwright harness.
import { expect, test } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('workbench panels and central tabs operate on the actual client', async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 })),
  );
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page.getByRole('heading', { name: '파일 검색', exact: true })).toBeVisible();
  const library = page.locator('#workspace-library');
  const toggle = page.locator('.workspace-library-toggle');
  await expect(library).toBeVisible();
  await toggle.click();
  await expect(library).toBeHidden();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click();
  await expect(library).toBeVisible();
  const tabs = page.getByRole('navigation', { name: 'Prism', exact: true });
  await tabs.getByRole('button', { name: '문서', exact: true }).click();
  await expect(tabs.getByRole('button', { name: '문서', exact: true })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await tabs.getByRole('button', { name: '검색', exact: true }).click();
  await page.getByRole('button', { name: '색인 상태', exact: true }).click();
  await expect(page.getByRole('heading', { name: '백그라운드 작업', exact: true })).toBeVisible();
  await page.locator('#indexing-tasks').getByRole('button', { name: '닫기', exact: true }).click();
  await expect(page.locator('#indexing-tasks')).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const theme of ['light', 'dark'] as const) {
  test(`IDE frame fits large English text and mobile tools in ${theme}`, async ({ page }) => {
    await page.addInitScript(
      (theme) =>
        localStorage.setItem(
          'prism.settings.v1',
          JSON.stringify({ onboardingVersion: 1, language: 'en', theme, fontSize: 18 }),
        ),
      theme,
    );
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'File search', exact: true })).toBeVisible();
    await mkdir(join(tmpdir(), 'prism-ui-evidence'), { recursive: true });
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 840 });
      await expect(page.locator('.workspace-tabs')).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(
        await page.locator('.workspace-editor').evaluate((el) => el.scrollWidth <= el.clientWidth),
      ).toBe(true);
      if (width === 390) {
        const library = page.locator('#workspace-library');
        const toggle = page.locator('.workspace-library-toggle');
        await expect(library).toBeHidden();
        await toggle.click();
        await expect(library).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');
        await page.keyboard.press('Escape');
        await expect(library).toBeHidden();
        await expect(toggle).toBeFocused();
      }
      await page.screenshot({
        path: join(tmpdir(), 'prism-ui-evidence', `ide-workspace-${theme}-${width}.png`),
      });
    }
    expect(errors).toEqual([]);
  });
}
