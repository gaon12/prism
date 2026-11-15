// Real settings UI, native persistence, filename collection and watcher/restart.
// Browser plugin not available: existing isolated WebView2/Playwright harness.
// Generated source/profile only; no content extraction, model or service mutation.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the source-qualified bundled executable');
const expectedRelease = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(expectedRelease), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1');
const originalExecutable = resolve(process.env.PRISM_TEST_EXECUTABLE);
const expectedSha = process.env.PRISM_TEST_APP_SHA256;
assert.match(expectedSha ?? '', /^[a-f0-9]{64}$/);
async function hash(path) {
  const value = createHash('sha256');
  for await (const chunk of createReadStream(path)) value.update(chunk);
  return value.digest('hex');
}
assert.equal(await hash(originalExecutable), expectedSha);
const root = await mkdtemp(join(tmpdir(), 'prism-native-file-policy-'));
const source = join(root, 'sources');
const application = join(root, 'app');
const data = join(application, 'PrismData');
const evidence = join(root, 'evidence');
await Promise.all([mkdir(source), mkdir(data, { recursive: true }), mkdir(evidence)]);
// A copied executable and portable marker isolate even an accidentally supplied
// release binary before it starts; it must never fall back to an installed profile.
const executable = join(application, 'prism.exe');
await copyFile(originalExecutable, executable);
await writeFile(join(application, 'prism.portable'), '');
assert.equal(await hash(executable), expectedSha);
await Promise.all([mkdir(join(source, '.git')), mkdir(join(source, '.hidden'))]);
const files = [
  'inclusionneedle-ordinary.md',
  'inclusionneedle-document.docx',
  'inclusionneedle-code.rs',
  'inclusionneedle-archive.zip',
  'inclusionneedle-ignored.md',
  '.hidden/inclusionneedle-hidden.md',
];
for (const name of files) await writeFile(join(source, name), 'Generated filename-only fixture.');
await writeFile(join(source, '.gitignore'), 'inclusionneedle-ignored.md\n');
const sourceHashes = Object.fromEntries(
  await Promise.all(files.map(async (name) => [name, await hash(join(source, name))])),
);
const scope = {
  id: 'owned',
  name: 'Owned fixture',
  path: source,
  filenameEnabled: true,
  contentEnabled: false,
  externalAllowed: false,
};
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F10',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    generation: { local: null, external: null },
    scopes: [scope],
  }),
);
const checks = [];
let instance;
let failure;
let lastResponse;
async function start() {
  instance = await launch(data, {
    executable,
    portable: true,
    productionPortable: expectedRelease === '1',
    cdpPort: process.env.PRISM_APP_CDP_PORT ? Number(process.env.PRISM_APP_CDP_PORT) : undefined,
  });
  const runtime = await instance.invoke('runtime_info');
  assert.equal(runtime.native, true);
  assert.equal(runtime.development, expectedRelease !== '1');
  assert.equal(runtime.installKind, 'portable');
  if (expectedRelease === '0')
    assert.equal((await instance.invoke('paste_fixture')).clipboard, null);
  const settings = await instance.invoke('get_settings');
  assert.equal(settings.clipboard.enabled, false);
  assert.equal(settings.scopes[0].id, scope.id);
  assert.equal(settings.scopes[0].path, source);
  await until(
    async () => (await instance.invoke('index_status')).initialScanComplete,
    'owned filename sweep',
  );
  checks.push({ name: 'isolated native startup', runtime });
}
async function names() {
  const response = await instance.invoke('search_files', {
    query: 'inclusionneedle-',
    extension: '',
    scopeId: 'owned',
  });
  lastResponse = response;
  return response.results.map((row) => basename(row.path)).sort();
}
async function expectNames(expected) {
  await until(
    async () => JSON.stringify(await names()) === JSON.stringify([...expected].sort()),
    `filename membership: ${expected.join(', ')}`,
  );
  assert.deepEqual(await names(), [...expected].sort());
}
async function policyDialog() {
  await instance.invoke('open_settings', { tab: 'scopes' });
  const dialog = instance.main.getByRole('dialog', { name: '설정', exact: true });
  const policy = dialog.locator('.file-policy-settings');
  await expect(policy.getByRole('checkbox')).toHaveCount(10);
  return {
    dialog,
    policy,
    filenames: policy.getByRole('group', { name: '파일명 검색', exact: true }),
    content: policy.getByRole('group', { name: '본문·AI 색인', exact: true }),
  };
}
async function capture(name, policy) {
  await policy.scrollIntoViewIfNeeded();
  await instance.main.screenshot({ path: join(evidence, name) });
}
try {
  await start();
  const all = files.map((name) => basename(name));
  await expectNames(all);
  let { dialog, policy, filenames, content } = await policyDialog();
  for (const control of await policy.getByRole('checkbox').all())
    await expect(control).toBeChecked();
  for (const control of await filenames.getByRole('checkbox').all()) await control.uncheck();
  await content.getByRole('checkbox', { name: '압축 파일', exact: true }).uncheck();
  await capture('native-policy-edited.png', policy);
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const allowed = ['inclusionneedle-document.docx', 'inclusionneedle-ordinary.md'];
  await expectNames(allowed);
  const saved = await instance.invoke('get_settings');
  assert.deepEqual(saved.scopes, [scope]);
  assert(Object.values(saved.filePolicy.filename).every((value) => value === false));
  assert.deepEqual(saved.filePolicy.content, {
    hidden: true,
    system: true,
    sourceCode: true,
    repositoryIgnored: true,
    archives: false,
  });
  assert.deepEqual(
    JSON.parse(await readFile(join(data, 'settings.json'), 'utf8')).filePolicy,
    saved.filePolicy,
  );
  checks.push({
    name: 'UI saves independent native rules, preserves permissions and recollects',
    policy: saved.filePolicy,
  });

  ({ dialog, policy, filenames } = await policyDialog());
  await filenames.getByRole('checkbox', { name: '소스 코드', exact: true }).focus();
  await instance.main.keyboard.press('Space');
  await expect(filenames.getByRole('checkbox', { name: '소스 코드', exact: true })).toBeChecked();
  await dialog.getByRole('button', { name: '취소', exact: true }).click();
  assert.deepEqual((await instance.invoke('get_settings')).filePolicy, saved.filePolicy);
  await shutdown(instance);
  instance = undefined;
  await start();
  assert.deepEqual((await instance.invoke('get_settings')).filePolicy, saved.filePolicy);
  await expectNames(allowed);
  checks.push({ name: 'cancel and real process restart preserve saved rules' });

  await writeFile(join(source, 'inclusionneedle-added.rs'), 'Excluded filename watcher fixture.');
  await writeFile(join(source, 'inclusionneedle-added.md'), 'Included filename watcher fixture.');
  await expectNames([...allowed, 'inclusionneedle-added.md']);
  await writeFile(join(source, '.gitignore'), '');
  await expectNames([...allowed, 'inclusionneedle-added.md', 'inclusionneedle-ignored.md']);
  checks.push({ name: 'real watcher prunes new code and reapplies changed repository rules' });

  ({ dialog, policy, filenames } = await policyDialog());
  for (const control of await filenames.getByRole('checkbox').all()) await control.check();
  await capture('native-policy-restored.png', policy);
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expectNames([...all, 'inclusionneedle-added.md', 'inclusionneedle-added.rs']);
  assert.deepEqual(
    (await instance.invoke('get_settings')).filePolicy.content,
    saved.filePolicy.content,
  );
  for (const [name, digest] of Object.entries(sourceHashes))
    assert.equal(await hash(join(source, name)), digest);
  assert.equal(await hash(executable), expectedSha);
  assert.equal(await hash(originalExecutable), expectedSha);
  assert.deepEqual(instance.errors, []);
  checks.push({
    name: 'restoring filename inclusion rediscovers pruned entries without changing content policy or sources',
  });
  for (const [language, title] of [
    ['ko', '포함할 항목'],
    ['en', 'Items to include'],
    ['zh-CN', '包含项目'],
    ['zh-TW', '包含項目'],
    ['ja', '含める項目'],
  ]) {
    const settings = await instance.invoke('get_settings');
    await instance.invoke('save_settings', {
      settings: { ...settings, language },
      expectedRevision: settings.revision,
    });
    await instance.invoke('open_settings', { tab: 'scopes' });
    const localized = instance.main.locator('.settings-modal[open] .file-policy-settings');
    await expect(localized.getByRole('heading', { name: title, exact: true })).toBeVisible();
    await expect(localized.getByRole('checkbox')).toHaveCount(10);
    await capture(`native-policy-${language}.png`, localized);
    await instance.main.keyboard.press('Escape');
    await expect(instance.main.locator('.settings-modal[open]')).toHaveCount(0);
  }
  checks.push({ name: 'native inclusion controls render in all five supported languages' });
  assert.deepEqual(instance.errors, []);
} catch (error) {
  failure = { message: String(error), stack: error.stack, lastResponse };
  await instance?.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
} finally {
  if (instance) await shutdown(instance);
  const result = {
    passed: !failure,
    root,
    executable,
    executableSha256: expectedSha,
    expectedRelease,
    checks,
    failure,
    limits: [
      'Owned small ordinary filesystem scope only; no real NTFS attributed transfer/fallback, native macOS, performance or 512MB qualification.',
      'Content permission stays disabled in the fixture; filename filtering does not grant extraction or external AI access.',
    ],
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
}
if (failure) process.exitCode = 1;
