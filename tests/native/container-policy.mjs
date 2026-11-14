// Real native policy UI and extraction of ZIP-based document formats.
// Browser plugin unavailable: use the existing isolated WebView/Playwright harness.
// Public DOCX, generated EPUB/ZIP and manual HWPX; no model or response replacement.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
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
const fixtures = fileURLToPath(new URL('../fixtures/', import.meta.url));
const docxFixture = join(fixtures, 'encrypted-office', 'example.docx');
const epubFixture = join(fixtures, 'epub', 'spine.epub');
const docxMetadata = JSON.parse(
  await readFile(join(fixtures, 'encrypted-office', 'sources.json'), 'utf8'),
).find((entry) => entry.file === 'example.docx');
assert(docxMetadata, 'Public plaintext DOCX must have independent provenance');
assert.equal(await hash(docxFixture), docxMetadata.sha256.toLowerCase());
const fixtureHashes = { docx: await hash(docxFixture), epub: await hash(epubFixture) };
const root = await mkdtemp(join(tmpdir(), 'prism-native-container-policy-'));
const source = join(root, 'sources');
const data = join(root, 'profile');
const evidence = join(root, 'evidence');
await Promise.all([mkdir(source), mkdir(data), mkdir(evidence)]);
const names = {
  docx: 'containerfn-document.docx',
  hwpx: 'containerfn-document.hwpx',
  epub: 'containerfn-book.epub',
  zip: 'containerfn-archive.zip',
  note: 'containerfn-note.md',
};
await Promise.all([
  copyFile(docxFixture, join(source, names.docx)),
  copyFile(epubFixture, join(source, names.epub)),
  // An actual ZIP package, not a text file bearing a ZIP suffix.
  copyFile(epubFixture, join(source, names.zip)),
  writeFile(join(source, names.note), 'Lorem ipsum. Generated ordinary document fixture.'),
]);
const scope = {
  id: 'owned',
  name: 'Container fixture',
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
const checks = [];
const sourceHashes = {};
let instance, failure, initialGeneration, lastContent;
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
  checks.push({ name: 'isolated native startup', runtime, webview: instance.browser.version() });
}
async function filenames() {
  const response = await instance.invoke('search_files', {
    query: 'containerfn-',
    extension: '',
    scopeId: 'owned',
  });
  return response.results.map((entry) => basename(entry.path)).sort();
}
async function expectFilenames(expected) {
  await until(
    async () => JSON.stringify(await filenames()) === JSON.stringify([...expected].sort()),
    'container filename membership',
  );
  assert.deepEqual(await filenames(), [...expected].sort());
}
async function contentNames() {
  const result = await instance.main.evaluate(
    async (args) => {
      try {
        return { value: await window.__TAURI_INTERNALS__.invoke('search_content', args) };
      } catch (error) {
        return { error };
      }
    },
    { query: 'ipsum', extension: '', scopeId: 'owned', requestId: randomUUID() },
  );
  assert.equal(
    result.error,
    undefined,
    `Native keyword search failed: ${JSON.stringify(result.error)}`,
  );
  lastContent = result.value;
  assert.equal(lastContent.hardLimitReached, false);
  return [...new Set(lastContent.hits.map((hit) => basename(hit.chunk.filePath)))].sort();
}
async function expectContent(expected) {
  await until(
    async () => JSON.stringify(await contentNames()) === JSON.stringify([...expected].sort()),
    'actual DOCX/HWPX/Markdown content membership',
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
    scope: modal.locator('.scope-setting').filter({ hasText: scope.name }),
  };
}
async function save(ui) {
  const previous = await instance.invoke('get_settings');
  await ui.modal.getByRole('button', { name: '저장', exact: true }).click();
  await expect(ui.modal).toHaveCount(0);
  const settings = await instance.invoke('get_settings');
  assert(settings.revision > previous.revision);
  assert.deepEqual(settings.generation, initialGeneration);
  assert.equal(settings.scopes[0].externalAllowed, false);
  assert.deepEqual(
    JSON.parse(await readFile(join(data, 'settings.json'), 'utf8')).filePolicy,
    settings.filePolicy,
  );
  return settings;
}
try {
  await start();
  const created = await instance.invoke('doc_create', {
    input: {
      title: 'Container policy fixture',
      purpose: 'manual fixture',
      scopeIds: [],
      templateMarkdown: null,
    },
  });
  const revision = await instance.invoke('doc_save_manual', {
    id: created.document.id,
    expected: created.revision.id,
    plan: null,
    blocks: [
      {
        id: 'owned',
        sectionId: null,
        kind: 'userInput',
        markdown: 'Lorem ipsum. Generated HWPX document fixture.',
        sourceLabels: [],
        claimsVerified: false,
        userEdited: true,
      },
    ],
  });
  await instance.invoke('doc_export_hwpx', {
    id: created.document.id,
    revision: revision.id,
    path: join(source, names.hwpx),
    request: randomUUID(),
    includeSources: false,
    includeTypeMarkers: false,
  });
  for (const name of Object.values(names)) sourceHashes[name] = await hash(join(source, name));
  for (const name of [names.docx, names.hwpx, names.epub, names.zip]) {
    const bytes = await readFile(join(source, name));
    assert.equal(bytes.readUInt32LE(0), 0x04034b50, `${name} must be a real ZIP package`);
  }
  assert.deepEqual((await instance.invoke('get_settings')).scopes, [scope]);
  await expectFilenames(Object.values(names));
  await expectContent([]);
  assert.equal((await instance.invoke('content_status')).indexed, 0);
  checks.push({
    name: 'actual document export and filename collection do not grant content access',
  });

  let ui = await dialog();
  await ui.filename.getByRole('checkbox', { name: '압축 파일', exact: true }).uncheck();
  await ui.body.getByRole('checkbox', { name: '압축 파일', exact: true }).uncheck();
  await ui.policy.scrollIntoViewIfNeeded();
  await instance.main.screenshot({ path: join(evidence, 'archive-excluded.png') });
  const restricted = await save(ui);
  assert.deepEqual(restricted.scopes, [scope]);
  const documents = [names.note, names.docx, names.hwpx];
  const withoutZip = [...documents, names.epub];
  await expectFilenames(withoutZip);
  await expectContent([]);
  checks.push({
    name: 'archive exclusion hides actual ZIP while preserving DOCX/HWPX/EPUB filenames',
  });

  ui = await dialog();
  await ui.scope.getByRole('checkbox').nth(1).check();
  const granted = await save(ui);
  assert.deepEqual(granted.scopes, [{ ...scope, contentEnabled: true }]);
  assert.deepEqual(granted.filePolicy, restricted.filePolicy);
  await expectContent(documents);
  await until(
    async () => (await instance.invoke('content_status')).phase === 'ready',
    'content collection complete',
  );
  await expectFilenames(withoutZip);
  checks.push({
    name: 'explicit content grant extracts actual DOCX and HWPX while archives remain excluded',
    content: lastContent,
  });

  await shutdown(instance);
  instance = undefined;
  await start();
  assert.deepEqual((await instance.invoke('get_settings')).filePolicy, restricted.filePolicy);
  await expectFilenames(withoutZip);
  await expectContent(documents);
  const cachedIds = lastContent.hits.map((hit) => hit.chunk.id);
  assert(cachedIds.length >= documents.length);
  checks.push({
    name: 'actual process restart preserves independent archive policy and document content',
  });

  ui = await dialog();
  await ui.filename.getByRole('checkbox', { name: '압축 파일', exact: true }).check();
  const restored = await save(ui);
  assert.equal(restored.filePolicy.filename.archives, true);
  assert.equal(restored.filePolicy.content.archives, false);
  await expectFilenames(Object.values(names));
  await expectContent(documents);
  ui = await dialog();
  await ui.scope.getByRole('checkbox').nth(1).uncheck();
  await ui.policy.scrollIntoViewIfNeeded();
  await instance.main.screenshot({ path: join(evidence, 'content-revoked.png') });
  await save(ui);
  await expectContent([]);
  assert.deepEqual(await instance.invoke('content_chunks', { ids: cachedIds }), []);
  await expectFilenames(Object.values(names));
  checks.push({ name: 'filename archive re-inclusion and content revocation remain independent' });
  assert.equal((await instance.invoke('content_status')).embedded, 0);
  assert.equal((await instance.invoke('model_status')).phase, 'notInstalled');
  assert.deepEqual(instance.errors, []);
} catch (error) {
  failure = { message: String(error), stack: error.stack, lastContent };
  await instance?.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
} finally {
  try {
    if (instance) await shutdown(instance);
    for (const [name, digest] of Object.entries(sourceHashes))
      assert.equal(await hash(join(source, name)), digest);
    assert.equal(await hash(docxFixture), fixtureHashes.docx);
    assert.equal(await hash(epubFixture), fixtureHashes.epub);
    assert.equal(await hash(executable), expectedSha);
  } catch (error) {
    failure ??= {
      message: `Owned app cleanup or source preservation failed: ${error}`,
      stack: error.stack,
    };
  }
  const result = {
    passed: !failure,
    root,
    executable,
    executableSha256: expectedSha,
    fixtureHashes,
    sourceHashes,
    checks,
    failure,
    limits: [
      'Owned Windows ordinary-filesystem scope only; no deployed NTFS, native macOS, full Linux app, performance or 512MB qualification.',
      'EPUB is checked for filename classification only; EPUB/ZIP content extraction and document viewer/layout fidelity are not claimed.',
      'HWPX is generated from a manual block without provider calls; no model inference or external transmission is configured.',
    ],
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
}
if (failure) process.exitCode = 1;
