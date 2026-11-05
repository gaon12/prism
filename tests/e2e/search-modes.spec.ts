// Actual client controls. Browser plugin not available: repository Playwright.
import { expect, test } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

test('keyboard mode selection preserves the query and utility routing', async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 })),
  );
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  const group = page.getByRole('group', { name: '검색 방식', exact: true });
  const automatic = group.getByRole('button', { name: '자동', exact: true });
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('한국어 pdf');
  await automatic.focus();
  await automatic.press('ArrowRight');
  const filename = group.getByRole('button', { name: '파일명', exact: true });
  await expect(filename).toBeFocused();
  await expect(filename).toHaveAttribute('aria-pressed', 'true');
  await expect(input).toHaveValue('한국어 pdf');
  await filename.press('End');
  const semantic = group.getByRole('button', { name: '자연어', exact: true });
  await expect(semantic).toBeFocused();
  await expect(semantic).toHaveAttribute('aria-pressed', 'true');
  await semantic.press('Tab');
  await expect(page.getByRole('button', { name: '필터', exact: true })).toBeFocused();
  await semantic.focus();
  await semantic.press('Home');
  await expect(automatic).toBeFocused();
  await expect(automatic).toHaveAccessibleDescription(/파일명을 먼저 찾고/);
  await input.fill('=1+1');
  await expect(group.locator('[aria-pressed="true"]')).toHaveCount(0);
  await group.getByRole('button', { name: '본문', exact: true }).click();
  await expect(input).toHaveValue('1+1');
  await expect(group.getByRole('button', { name: '본문', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(errors).toEqual([]);
});

for (const theme of ['light', 'dark'] as const) {
  test(`segmented labels fit large English text in ${theme} and respect reduced motion`, async ({
    page,
  }) => {
    await page.addInitScript(
      (theme) =>
        localStorage.setItem(
          'prism.settings.v1',
          JSON.stringify({ onboardingVersion: 1, language: 'en', fontSize: 18, theme }),
        ),
      theme,
    );
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto('/');
    await expect(page).toHaveTitle('Prism');
    const group = page.getByRole('group', { name: 'Search mode', exact: true });
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 840 });
      await expect(group).toBeVisible();
      await group.getByRole('button', { name: 'Natural language', exact: true }).click();
      const bounds = await group.boundingBox();
      expect(bounds).not.toBeNull();
      for (const button of await group.getByRole('button').all()) {
        const box = await button.boundingBox();
        expect(box!.x).toBeGreaterThanOrEqual(bounds!.x);
        expect(box!.x + box!.width).toBeLessThanOrEqual(bounds!.x + bounds!.width + 0.5);
        expect(box!.x + box!.width).toBeLessThanOrEqual(width);
        expect(await button.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
          true,
        );
      }
      const selected = await group
        .getByRole('button', { name: 'Natural language', exact: true })
        .boundingBox();
      const thumb = group.locator('.ss-seg__thumb');
      expect(Math.abs((await thumb.boundingBox())!.x - selected!.x)).toBeLessThan(0.75);
      expect(await thumb.evaluate((element) => getComputedStyle(element).transitionDuration)).toBe(
        '0s',
      );
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      await mkdir(join(tmpdir(), 'prism-ui-evidence'), { recursive: true });
      await page.screenshot({
        path: join(tmpdir(), 'prism-ui-evidence', `stageshow-modes-${theme}-${width}.png`),
      });
    }
    expect(errors).toEqual([]);
  });
}
