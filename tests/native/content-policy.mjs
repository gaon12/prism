// Actual settings UI -> content collection/SQLite -> keyword results/chunks.
// Browser plugin not available: the existing isolated WebView2 harness is used.
// Only generated documents/profile; no model, provider, opener or service action.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the source-qualified bundled debug executable');
const executable = resolve(process.env.PRISM_TEST_EXECUTABLE);
const expectedSha = process.env.PRISM_TEST_APP_SHA256;
assert.match(expectedSha ?? '', /^[a-f0-9]{64}$/);
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
assert.equal(await hash(executable), expectedSha);
const root = await mkdtemp(join(tmpdir(), 'prism-native-content-policy-'));
const source = join(root, 'sources');
const data = join(root, 'profile');
const evidence = join(root, 'evidence');
await Promise.all([mkdir(source), mkdir(data), mkdir(evidence)]);
await Promise.all(
  ['.git', '.hidden', 'generated', 'system-owned'].map((name) => mkdir(join(source, name))),
);
const files = [
  'policyfn-note.md',
  'policyfn-code.rs',
  '.hidden/policyfn-hidden.md',
  'generated/policyfn-ignored.md',
  'policyfn-system.md',
  'system-owned/policyfn-system-child.md',
];
for (const name of files)
  await writeFile(join(source, name), `ownedcontentneedle generated evidence for ${name}`);
await writeFile(join(source, '.gitignore'), '/generated/\n');
const sourceHashes = Object.fromEntries(
  await Promise.all(files.map(async (name) => [name, await hash(join(source, name))])),
);
const scope = {
  id: 'owned',
  name: 'Policy fixture',
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
    rememberSearch: false,
    shortcut: 'Ctrl+Alt+Shift+F10',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    automaticSemantic: false,
    generation: { local: null, external: null },
    scopes: [scope],
  }),
);
const attributeTargets = [join(source, 'policyfn-system.md'), join(source, 'system-owned')];
const systemChild = join(source, 'system-owned', 'policyfn-system-child.md');
function systemAttributes(path, flags) {
  assert(attributeTargets.includes(path) || (path === systemChild && flags === undefined));
  if (flags !== undefined) assert(Number.isInteger(flags) && flags >= 0 && flags <= 0xffffffff);
  return JSON.parse(
    execFileSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        '$p=$env:PRISM_POLICY_OWNED_FILE; $before=[int][IO.File]::GetAttributes($p); if ($env:PRISM_POLICY_FLAGS) { [IO.File]::SetAttributes($p,[IO.FileAttributes][int]$env:PRISM_POLICY_FLAGS) }; @{ before=$before; after=[int][IO.File]::GetAttributes($p) } | ConvertTo-Json -Compress',
      ],
      {
        windowsHide: true,
        timeout: 15_000,
        encoding: 'utf8',
        env: {
          ...process.env,
          PRISM_POLICY_OWNED_FILE: path,
          PRISM_POLICY_FLAGS: flags === undefined ? '' : String(flags),
        },
      },
    ),
  );
}
const checks = [];
let instance, failure, lastContent, initialGeneration;
const originalAttributes = [];
let request = 0;
async function start() {
  instance = await launch(data, {
    executable,
    cdpPort: process.env.PRISM_APP_CDP_PORT ? Number(process.env.PRISM_APP_CDP_PORT) : undefined,
  });
  const runtime = await instance.invoke('runtime_info');
  assert.equal(runtime.native, true);
  assert.equal(runtime.development, true);
  assert.equal(runtime.installKind, 'isolated');
  assert.equal((await instance.invoke('paste_fixture')).clipboard, null);
  const settings = await instance.invoke('get_settings');
  initialGeneration ??= settings.generation;
  assert.equal(initialGeneration.local, null);
  assert.equal(initialGeneration.external, null);
  assert.equal(initialGeneration.allowExternalQuestions, false);
  assert.deepEqual(settings.generation, initialGeneration);
  await instance.main.getByRole('textbox', { name: '검색어', exact: true }).fill('');
  checks.push({
    name: 'owned native startup',
    runtime,
    webviewVersion: instance.browser.version(),
    viewport: await instance.main.evaluate(() => ({
      width: innerWidth,
      height: innerHeight,
      scale: devicePixelRatio,
    })),
  });
}
async function content() {
  const args = {
    query: 'ownedcontentneedle',
    extension: '',
    scopeId: 'owned',
    requestId: `policy-${++request}`,
  };
  const result = await instance.main.evaluate(async (args) => {
    try {
      return { value: await window.__TAURI_INTERNALS__.invoke('search_content', args) };
    } catch (error) {
      return { error };
    }
  }, args);
  if (result.error)
    throw new Error(`Native keyword search rejected: ${JSON.stringify(result.error)}`);
  lastContent = result.value;
  assert.equal(lastContent.hardLimitReached, false);
  return lastContent;
}
async function contentNames() {
  return [...new Set((await content()).hits.map((hit) => basename(hit.chunk.filePath)))].sort();
}
async function filenames() {
  const response = await instance.invoke('search_files', {
    query: 'policyfn-',
    extension: '',
    scopeId: 'owned',
  });
  return response.results.map((entry) => basename(entry.path)).sort();
}
async function expectFilenames(expected) {
  await until(
    async () => JSON.stringify(await filenames()) === JSON.stringify([...expected].sort()),
    'native filename membership after policy change',
  );
  assert.deepEqual(await filenames(), [...expected].sort());
}
async function expectContent(expected) {
  await until(
    async () =>
      JSON.stringify(await contentNames()) === JSON.stringify([...expected].sort()) &&
      (await instance.invoke('content_status')).phase === 'ready',
    'native content membership and completed collection',
  );
  assert.deepEqual(await contentNames(), [...expected].sort());
}
async function dialog() {
  await instance.invoke('open_settings', { tab: 'scopes' });
  const modal = instance.main.getByRole('dialog', { name: '설정', exact: true });
  const policy = modal.locator('.file-policy-settings');
  await expect(policy.getByRole('checkbox')).toHaveCount(10);
  return {
    modal,
    policy,
    filename: policy.getByRole('group', { name: '파일명 검색', exact: true }),
    body: policy.getByRole('group', { name: '본문·AI 색인', exact: true }),
    scope: modal.locator('.scope-setting').filter({ hasText: 'Policy fixture' }),
  };
}
async function save(modal) {
  const before = await instance.invoke('get_settings');
  await modal.getByRole('button', { name: '저장', exact: true }).click();
  await expect(modal).toHaveCount(0);
  const settings = await instance.invoke('get_settings');
  assert(settings.revision > before.revision, 'UI save must commit a newer native revision');
  assert.equal(settings.scopes[0].externalAllowed, false);
  assert.deepEqual(settings.generation, initialGeneration);
  assert.deepEqual(
    JSON.parse(await readFile(join(data, 'settings.json'), 'utf8')).filePolicy,
    settings.filePolicy,
  );
  return settings;
}
async function capture(name, policy) {
  await policy.scrollIntoViewIfNeeded();
  await instance.main.screenshot({ path: join(evidence, name) });
}
try {
  for (const path of attributeTargets) {
    const before = systemAttributes(path).before;
    originalAttributes.push({ path, flags: before });
    assert.equal(systemAttributes(path, before | 4).after & 4, 4);
  }
  assert.equal(systemAttributes(systemChild).after & 4, 0, 'Child has no SYSTEM flag of its own');
  checks.push({
    name: 'owned SYSTEM file and directory attributes set; directory child is ordinary',
    originalAttributes,
  });
  await start();
  await until(
    async () => (await instance.invoke('index_status')).initialScanComplete,
    'initial filename collection',
  );
  const all = files.map((name) => basename(name));
  assert.deepEqual(await filenames(), [...all].sort());
  await expectContent([]);
  let ui = await dialog();
  await ui.scope.getByRole('checkbox').nth(1).check();
  await ui.filename.getByRole('checkbox', { name: '소스 코드', exact: true }).uncheck();
  const first = await save(ui.modal);
  assert.equal(first.scopes[0].contentEnabled, true);
  await expectContent(all);
  const withoutCode = all.filter((name) => !name.endsWith('.rs')).sort();
  assert.deepEqual(await filenames(), withoutCode);
  const cached = (await content()).hits.map((hit) => hit.chunk);
  assert.equal(cached.length, 6);
  checks.push({
    name: 'explicit folder content grant indexes all six documents including SYSTEM-folder child independently of filename source-code exclusion',
    names: await contentNames(),
  });

  ui = await dialog();
  await ui.filename.getByRole('checkbox', { name: '시스템 항목', exact: true }).uncheck();
  await save(ui.modal);
  await expectFilenames(withoutCode.filter((name) => !name.startsWith('policyfn-system')));
  await expectContent(all);
  ui = await dialog();
  await ui.filename.getByRole('checkbox', { name: '시스템 항목', exact: true }).check();
  await save(ui.modal);
  await expectFilenames(withoutCode);
  await expectContent(all);
  checks.push({
    name: 'filename SYSTEM exclusion removes both owned file and ordinary folder descendant, and re-inclusion restores both without changing content access',
  });

  ui = await dialog();
  for (const checkbox of await ui.body.getByRole('checkbox').all()) await checkbox.uncheck();
  await capture('native-content-policy-restricted.png', ui.policy);
  const restricted = await save(ui.modal);
  assert.equal(restricted.filePolicy.filename.sourceCode, false);
  assert(Object.values(restricted.filePolicy.content).every((value) => value === false));
  const denied = cached
    .filter((chunk) => !chunk.filePath.endsWith('policyfn-note.md'))
    .map((chunk) => chunk.id);
  assert.equal(denied.length, 5);
  assert.deepEqual(await instance.invoke('content_chunks', { ids: denied }), []);
  await expectContent(['policyfn-note.md']);
  assert.deepEqual(await filenames(), withoutCode);
  await instance.main.getByRole('button', { name: '본문', exact: true }).click();
  await instance.main
    .getByRole('textbox', { name: '검색어', exact: true })
    .fill('ownedcontentneedle');
  await expect(instance.main.locator('.content-results').getByRole('option')).toHaveCount(1);
  await expect(instance.main.locator('.content-results')).toContainText('policyfn-note.md');
  await instance.main.screenshot({ path: join(evidence, 'native-content-policy-results.png') });
  checks.push({
    name: 'UI restriction removes hidden/system/code/ignored cached hits and stale chunk IDs while filename results remain independent',
    policy: restricted.filePolicy,
  });

  await shutdown(instance);
  instance = undefined;
  await start();
  assert.deepEqual((await instance.invoke('get_settings')).filePolicy, restricted.filePolicy);
  await expectContent(['policyfn-note.md']);
  assert.deepEqual(await instance.invoke('content_chunks', { ids: denied }), []);
  checks.push({
    name: 'actual process restart preserves restricted content admission and stale-chunk rejection',
  });
  ui = await dialog();
  for (const checkbox of await ui.body.getByRole('checkbox').all()) await checkbox.check();
  const restored = await save(ui.modal);
  assert.deepEqual(restored.filePolicy.filename, restricted.filePolicy.filename);
  assert(Object.values(restored.filePolicy.content).every((value) => value === true));
  await expectContent(all);
  assert.deepEqual(await filenames(), withoutCode);
  ui = await dialog();
  await ui.scope.getByRole('checkbox').nth(1).uncheck();
  await save(ui.modal);
  assert.deepEqual(
    await instance.invoke('content_chunks', { ids: cached.map((chunk) => chunk.id) }),
    [],
  );
  await expectContent([]);
  assert.deepEqual(await filenames(), withoutCode);
  checks.push({
    name: 'UI re-inclusion restores cached content and later folder revocation hides all content/chunks without changing filename access',
  });
  for (const [name, digest] of Object.entries(sourceHashes))
    assert.equal(await hash(join(source, name)), digest);
  assert.equal(await hash(executable), expectedSha);
  assert.deepEqual(instance.errors, []);
  assert.equal((await instance.invoke('content_status')).embedded, 0);
} catch (error) {
  failure = { message: String(error), stack: error.stack, lastContent };
  await instance?.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
} finally {
  try {
    if (instance) await shutdown(instance);
  } catch (error) {
    failure ??= { message: `Owned app cleanup failed: ${error}` };
  }
  try {
    for (const { path, flags } of originalAttributes)
      assert.equal(systemAttributes(path, flags).after, flags);
  } catch (error) {
    failure ??= { message: `Owned fixture attributes could not be restored: ${error}` };
  }
  const result = {
    passed: !failure,
    root,
    executable,
    executableSha256: expectedSha,
    checks,
    failure,
    sourceHashes,
    limits: [
      'Generated small Windows ordinary-filesystem scope and keyword search only; no actual embeddings/provider/vector model or deployed NTFS transfer.',
      'No macOS/full Linux app, low-memory performance or native narrow-layout qualification.',
    ],
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
}
if (failure) process.exitCode = 1;
