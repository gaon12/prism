import { expect, test } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const evidence = join(tmpdir(), 'prism-ui-evidence');
test.beforeAll(() => mkdir(evidence, { recursive: true }));

test('reviews fresh settings without side effects and retries a failed completion save', async ({
  page,
}) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toHaveAccessibleName('Prism에 오신 것을 환영해요');
  await dialog.getByText('어둡게', { exact: true }).click();
  expect(await page.evaluate(() => localStorage.getItem('prism.settings.v1'))).toBeNull();
  // Modal focus stays inside the tour; background navigation is inert.
  await page.keyboard.press('Shift+Tab');
  expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true);
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await dialog.locator('.onboarding-recorder').click();
  await dialog.locator('.onboarding-recorder').press('KeyA');
  await expect(dialog.getByRole('alert')).toContainText('Ctrl, Alt 또는 Win');
  await dialog.locator('.onboarding-recorder').press('Control+Shift+KeyK');
  await expect(dialog.locator('.onboarding-keys')).toContainText('Ctrl');
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await expect(dialog.getByRole('heading')).toHaveText('어디를 검색할까요?');
  await expect(dialog.locator('.onboarding-folders li')).toHaveCount(0);
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await expect(dialog).toContainText('EmbeddingGemma 2');
  await expect(dialog.getByRole('switch', { name: /외부 AI 서비스에 질문/ })).not.toBeChecked();
  await dialog.getByText('클립보드 기록 보관', { exact: true }).click();
  await expect(dialog.getByRole('switch', { name: '복사한 이미지도 보관' })).toBeDisabled();
  await page.screenshot({ path: join(evidence, 'onboarding-privacy.png'), animations: 'disabled' });
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await page.evaluate(() => {
    const original = Storage.prototype.setItem;
    (window as unknown as { restoreStorage: () => void }).restoreStorage = () => {
      Storage.prototype.setItem = original;
    };
    Storage.prototype.setItem = function (key, value) {
      if (key === 'prism.settings.v1') throw new Error('Synthetic settings write failure');
      return original.call(this, key, value);
    };
  });
  await dialog.getByRole('button', { name: 'Prism 시작하기', exact: true }).click();
  await expect(dialog.getByRole('alert')).toHaveText('Synthetic settings write failure');
  await page.evaluate(() => (window as unknown as { restoreStorage: () => void }).restoreStorage());
  await dialog.getByRole('button', { name: 'Prism 시작하기', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(() => JSON.parse(localStorage.getItem('prism.settings.v1')!)),
  ).toMatchObject({
    onboardingVersion: 1,
    theme: 'dark',
    shortcut: 'Ctrl+Shift+K',
    scopes: [],
    clipboard: { enabled: false, captureImages: false, onboarded: true },
    generation: { allowExternalQuestions: false },
  });
  await page.reload();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('mobile skip discards draft choices and existing profiles bypass the tour', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  const dialog = page.getByRole('dialog');
  await dialog.getByText('English', { exact: true }).click();
  await expect(dialog).toHaveAccessibleName('Welcome to Prism');
  await page.screenshot({ path: join(evidence, 'onboarding-mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await dialog.getByRole('button', { name: 'Skip for now', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(() => JSON.parse(localStorage.getItem('prism.settings.v1')!)),
  ).toMatchObject({ onboardingVersion: 1, language: 'ko', scopes: [] });
  await page.evaluate(() =>
    localStorage.setItem('prism.settings.v1', JSON.stringify({ language: 'ja', theme: 'dark' })),
  );
  await page.reload();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.locator('html')).toHaveAttribute('lang', 'ja');
});
