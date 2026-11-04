// Actual bundled first-run UI and settings persistence; isolated MemoryClipboard.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-onboarding-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({ onboardingVersion: 0, shortcut: 'Ctrl+Alt+Shift+F3', startOnLogin: false }),
);
let instance;
try {
  instance = await launch(directory);
  let { main, invoke } = instance;
  const dialog = main.getByRole('dialog');
  await dialog.getByRole('heading', { name: 'Prism에 오신 것을 환영해요' }).waitFor();
  assert.equal(await dialog.evaluate((element) => element.matches(':modal')), true);
  await main.screenshot({ path: join(evidence, 'onboarding-native-welcome.png') });
  await dialog.getByText('어둡게', { exact: true }).click();
  assert.equal((await invoke('get_settings')).theme, 'system', 'Draft choices do not save early');
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await dialog.locator('.onboarding-recorder').click();
  await dialog.locator('.onboarding-recorder').press('Control+Alt+Shift+F4');
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await dialog.getByRole('heading', { name: '어디를 검색할까요?' }).waitFor();
  await until(
    async () =>
      !(await dialog.locator('.onboarding-content').innerText()).includes('추천 폴더를 찾는 중'),
    'recommended folder loading completes',
  );
  // Never authorize the test to scan a real suggested user folder.
  for (const checkbox of await dialog.locator('.onboarding-folder-name input').all())
    await checkbox.uncheck();
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  assert((await dialog.innerText()).includes('EmbeddingGemma 2'));
  assert.equal(
    await dialog.getByRole('switch', { name: /외부 AI 서비스에 질문/ }).isChecked(),
    false,
  );
  await dialog.getByText('클립보드 기록 보관', { exact: true }).click();
  await main.screenshot({
    path: join(evidence, 'onboarding-native-privacy.png'),
    animations: 'disabled',
  });
  await dialog.getByRole('button', { name: '다음', exact: true }).click();
  await dialog.getByRole('button', { name: 'Prism 시작하기', exact: true }).click();
  await until(async () => (await main.getByRole('dialog').count()) === 0, 'tour completion');
  const saved = await invoke('get_settings');
  assert.equal(saved.onboardingVersion, 1);
  assert.equal(saved.shortcut, 'Ctrl+Alt+Shift+F4');
  assert.equal(saved.theme, 'dark');
  assert.equal(saved.scopes.length, 0);
  assert.equal(saved.clipboard.enabled, false);
  assert.equal(saved.clipboard.onboarded, true);
  assert.equal(saved.generation.allowExternalQuestions, false);
  assert.equal(
    JSON.parse(await readFile(join(directory, 'settings.json'), 'utf8')).onboardingVersion,
    1,
  );
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory);
  ({ main, invoke } = instance);
  assert.equal(await main.getByRole('dialog').count(), 0);
  assert.equal((await invoke('get_settings')).onboardingVersion, 1);
  // Reset only the isolated completion marker and verify skipping discards choices.
  const previous = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...previous, onboardingVersion: 0 },
    expectedRevision: previous.revision,
  });
  await main
    .getByRole('dialog')
    .getByRole('heading', { name: 'Prism에 오신 것을 환영해요' })
    .waitFor();
  await main.getByRole('dialog').getByText('English', { exact: true }).click();
  await main.getByRole('dialog').getByRole('button', { name: 'Skip for now', exact: true }).click();
  await until(async () => (await main.getByRole('dialog').count()) === 0, 'skip persists');
  const skipped = await invoke('get_settings');
  assert.equal(skipped.onboardingVersion, 1);
  assert.equal(skipped.language, previous.language);
  assert.equal(skipped.theme, previous.theme);
  assert.deepEqual(skipped.scopes, previous.scopes);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({ passed: true, directory, realUserFoldersIndexed: 0, externalCalls: 0 }),
  );
} finally {
  if (instance) await shutdown(instance);
}
