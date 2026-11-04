import { expect, test } from '@playwright/test';
test('update preferences save explicitly and browser preview never fabricates releases', async ({
  page,
}) => {
  const requests: string[] = [];
  page.on('request', (request) => {
    if (request.url().includes('github.com')) requests.push(request.url());
  });
  await page.addInitScript(() =>
    localStorage.setItem(
      'prism.settings.v1',
      JSON.stringify({ onboardingVersion: 1, clipboard: { onboarded: true } }),
    ),
  );
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  let dialog = page.getByRole('dialog', { name: '설정', exact: true });
  let panel = dialog.getByRole('region', { name: '업데이트', exact: true });
  await expect(panel.getByLabel('새 버전 자동 확인', { exact: true })).toBeChecked();
  await expect(panel.getByRole('button', { name: '지금 확인', exact: true })).toBeDisabled();
  await expect(panel.locator('.update-notes')).toHaveCount(0);
  await panel.getByLabel('새 버전 자동 확인', { exact: true }).uncheck();
  expect(
    (await page.evaluate(() => JSON.parse(localStorage.getItem('prism.settings.v1')!))).updates,
  ).toBeUndefined();
  await dialog.getByRole('button', { name: '취소', exact: true }).click();
  await page.getByRole('button', { name: '설정', exact: true }).click();
  dialog = page.getByRole('dialog', { name: '설정', exact: true });
  panel = dialog.getByRole('region', { name: '업데이트', exact: true });
  await expect(panel.getByLabel('새 버전 자동 확인', { exact: true })).toBeChecked();
  await panel.getByLabel('새 버전 자동 확인', { exact: true }).uncheck();
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  expect(
    (await page.evaluate(() => JSON.parse(localStorage.getItem('prism.settings.v1')!))).updates
      .autoCheck,
  ).toBe(false);
  expect(requests).toEqual([]);
});
