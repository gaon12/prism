// Actual Office/password/extraction/SQLite services in an owned profile.
// No crypto, source, token or content result is fabricated.
// No model/provider is started.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, toNamespacedPath } from 'node:path';
import { expect } from '@playwright/test';
import { launch, shutdown, until } from './harness.mjs';
const directory = await mkdtemp(join(tmpdir(), 'prism-office-unlock-'));
const profile = join(directory, 'profile');
const source = join(directory, 'source'),
  privateFolder = join(directory, 'filename-only');
await mkdir(source);
await mkdir(privateFolder);
await mkdir(profile);
const cases = [
  ['example_password.docx', '암호 문서.docx', 'Password1234_', 'lorem', 'docx'],
  ['ecma376standard_password.docx', '이전 Office.docx', 'Password1234_', 'lorem', 'docx'],
  ['example_password.xlsx', '암호 표.xlsx', 'Password1234_', 'lorem', 'xlsx'],
  ['ecma376agile_sha1_password.xlsx', 'SHA1 표.xlsx', 'Password1234_', 'ipsum', 'xlsx'],
  [
    'agile-unicode-slides.pptx',
    '한글 암호 슬라이드.pptx',
    '암호-fixture-密碼🔐',
    'Test text',
    'pptx',
  ],
];
const originals = new Map();
for (const [fixture, name] of cases) {
  const path = join(source, name);
  await copyFile(join('tests/fixtures/encrypted-office', fixture), path);
  originals.set(path, await readFile(path));
}
const cancelled = join(source, '취소.docx'),
  outside = join(privateFolder, '외부.docx');
await copyFile('tests/fixtures/encrypted-office/example_password.docx', cancelled);
await copyFile('tests/fixtures/encrypted-office/example_password.docx', outside);
originals.set(cancelled, await readFile(cancelled));
originals.set(outside, await readFile(outside));
const pdf = join(source, '기존 PDF.pdf');
await copyFile('tests/fixtures/encrypted-pdf/aes256.pdf', pdf);
originals.set(pdf, await readFile(pdf));
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    shortcut: 'Ctrl+Alt+Shift+F7',
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'office',
        name: 'Office',
        path: source,
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
      {
        id: 'private',
        name: 'Filename only',
        path: privateFolder,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);
const options = process.env.PRISM_TEST_EXECUTABLE
  ? { executable: process.env.PRISM_TEST_EXECUTABLE }
  : {};
const pathKey = (value) =>
  process.platform === 'win32' ? toNamespacedPath(value).toLowerCase() : value;
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
let instance;
try {
  instance = await launch(profile, options);
  let { main, quick, invoke } = instance;
  const settings = await invoke('get_settings');
  await until(async () => {
    const status = await invoke('content_status');
    return (
      status.phase === 'ready' && status.issues.filter((i) => i.code === 'encrypted').length === 7
    );
  }, 'all protected Office and PDF sources listed');
  await invoke('content_cancel_index');
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.locator('.content-issues > summary').click();
  await main.getByRole('button', { name: '암호 입력: 암호 문서.docx', exact: true }).click();
  let dialog = main.getByRole('dialog', { name: '암호 Office 문서 다시 색인', exact: true });
  await expect(dialog.getByText(/원본 문서는 변경하지 않으며/)).toBeVisible();
  await dialog.getByLabel('문서 비밀번호', { exact: true }).fill('wrong-office-password-canary');
  await dialog.getByRole('button', { name: '암호로 색인', exact: true }).click();
  await expect(dialog.getByRole('alert')).toHaveText('암호가 일치하지 않습니다.');
  assert.equal(await dialog.getByLabel('문서 비밀번호', { exact: true }).inputValue(), '');
  assert.equal((await invoke('content_status')).indexed, 0);
  await main.screenshot({ path: join(directory, 'office-password-ko-light.png') });
  const beforeTheme = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...beforeTheme, language: 'en', theme: 'dark' },
    expectedRevision: beforeTheme.revision,
  });
  await main.setViewportSize({ width: 760, height: 760 });
  dialog = main.getByRole('dialog', {
    name: 'Index a password-protected Office document',
    exact: true,
  });
  await expect(dialog.getByLabel('Document password', { exact: true })).toBeVisible();
  assert(await dialog.evaluate((element) => element.getBoundingClientRect().right <= innerWidth));
  await main.screenshot({ path: join(directory, 'office-password-en-dark760.png') });
  await dialog.getByLabel('Document password', { exact: true }).fill(cases[0][2]);
  await dialog.getByRole('button', { name: 'Index with password', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  assert.equal(
    (await invoke('content_status')).phase,
    'cancelled',
    'manual Office retry preserves pause',
  );
  const rejected = (page, command, args) =>
    page.evaluate(
      async ({ command, args }) => {
        try {
          await window.__TAURI_INTERNALS__.invoke(command, args);
          return null;
        } catch (error) {
          return error.code;
        }
      },
      { command, args },
    );
  const prepare = () => invoke('content_prepare_unlock');
  let token = await prepare();
  assert.equal(
    await rejected(main, 'content_unlock_office', {
      path: outside,
      password: cases[0][2],
      requestId: token,
    }),
    'outsideScope',
  );
  token = await prepare();
  await invoke('content_cancel_unlock', { requestId: token });
  assert.equal(
    await rejected(main, 'content_unlock_office', {
      path: cancelled,
      password: cases[0][2],
      requestId: token,
    }),
    'cancelled',
  );
  // Quick-window input cannot consume a main-window retry token.
  token = await prepare();
  assert.equal(
    await rejected(quick, 'content_unlock_office', {
      path: cancelled,
      password: cases[0][2],
      requestId: token,
    }),
    'cancelled',
  );
  await invoke('content_cancel_unlock', { requestId: token });
  for (const [, name, password, , extension] of cases.slice(1)) {
    const path = join(source, name);
    token = await prepare();
    const summary = await invoke('content_unlock_office', { path, password, requestId: token });
    assert.equal(summary.format, extension);
    assert.equal(
      summary.fingerprint.sha256,
      hash(originals.get(path)),
      'fingerprint belongs to original encrypted bytes',
    );
    assert.equal((await invoke('content_status')).phase, 'cancelled');
  }
  const quickSummary = await quick.evaluate(
    async ({ path, password }) => {
      const requestId = await window.__TAURI_INTERNALS__.invoke('content_prepare_unlock');
      return window.__TAURI_INTERNALS__.invoke('content_unlock_office', {
        path,
        password,
        requestId,
      });
    },
    { path: join(source, cases[2][1]), password: cases[2][2] },
  );
  assert.equal(quickSummary.fingerprint.sha256, hash(originals.get(join(source, cases[2][1]))));
  // Preserve the existing PDF-specific IPC and the same shared retry admission.
  token = await prepare();
  assert.equal(
    await rejected(main, 'content_unlock_pdf', {
      path: cancelled,
      password: cases[0][2],
      requestId: token,
    }),
    'unsupportedFormat',
  );
  token = await prepare();
  await invoke('content_unlock_pdf', {
    path: pdf,
    password: '암호-fixture-密碼',
    requestId: token,
  });
  const search = (query, extension) =>
    invoke('search_content', {
      query,
      extension,
      scopeId: 'office',
      requestId: crypto.randomUUID(),
    });
  for (const [, name, , query, extension] of cases) {
    const result = await search(query, extension);
    const hit = result.hits.find(
      (hit) => pathKey(hit.chunk.filePath) === pathKey(join(source, name)),
    );
    assert(hit, `Missing decrypted Office content from ${name}`);
    assert.equal(hit.chunk.fingerprint.sha256, hash(originals.get(join(source, name))));
  }
  assert.equal((await search('unlockcanarythird', 'pdf')).hits.length, 1);
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  assert.deepEqual((await invoke('get_settings')).scopes, settings.scopes);
  for (const [path, bytes] of originals) assert.deepEqual(await readFile(path), bytes);
  const officeFiles = (await readdir(directory, { recursive: true, withFileTypes: true }))
    .filter((e) => e.isFile() && /\.(docx|xlsx|pptx|pdf)$/iu.test(e.name))
    .map((e) => join(e.parentPath, e.name));
  assert.deepEqual(
    officeFiles.sort(),
    [...originals.keys()].sort(),
    'no decrypted document is written',
  );
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;
  instance = await launch(profile, { ...options, readyHeading: false });
  ({ main, invoke } = instance);
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed >= 6;
  }, 'protected text remains cached after restart without passwords');
  const resumed = await invoke('search_content', {
    query: 'Test text',
    extension: 'pptx',
    scopeId: 'office',
    requestId: crypto.randomUUID(),
  });
  assert(
    resumed.hits.some((hit) => pathKey(hit.chunk.filePath) === pathKey(join(source, cases[4][1]))),
  );
  assert.equal((await invoke('model_status')).phase, 'notInstalled');
  const privateFiles = (await readdir(directory, { recursive: true, withFileTypes: true })).filter(
    (e) => e.isFile() && /\.(json|sqlite|db|log)(?:-(?:wal|shm))?$/iu.test(e.name),
  );
  for (const file of privateFiles) {
    const bytes = await readFile(join(file.parentPath, file.name));
    for (const password of ['wrong-office-password-canary', cases[0][2], cases[4][2]])
      assert(!bytes.includes(Buffer.from(password)), `Password persisted in ${file.name}`);
  }
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      directory,
      officeFormats: ['docx', 'xlsx', 'pptx'],
      independentCases: cases.length,
      encryptedFingerprints: true,
      sourcePreserved: true,
      pausedRetry: true,
      restart: true,
      embeddingsStarted: false,
      errors: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
