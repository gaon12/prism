// Actual packaged binary, portable paths, WebView storage, DPAPI and real local
// EmbeddingGemma 2/Lance retrieval. All files and clipboard access are isolated.
// Browser plugin not available: repository Playwright/WebView2 harness.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  stat,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';
import {
  loadPackageNotices,
  noticeNames,
  writePackageNotices,
} from '../../scripts/package-notices.mjs';

const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE ?? 'src-tauri/target/debug/prism.exe');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile), 'Expected profile must be 0 (debug) or 1 (release)');
const hashFile = async (path) => {
  const digest = createHash('sha256');
  for await (const bytes of createReadStream(path)) digest.update(bytes);
  return digest.digest('hex');
};
const candidateSha256 = await hashFile(candidate);
const notices = loadPackageNotices(candidate);

const directory = await mkdtemp(join(tmpdir(), 'prism-portable-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
const output = join(directory, 'packages');
const { version } = JSON.parse(await readFile(resolve('src-tauri/tauri.conf.json'), 'utf8'));
const name = `Prism_${version}_x64_portable`;
const preserved = join(output, name);
await mkdir(preserved, { recursive: true });
await writeFile(join(preserved, 'sentinel.txt'), 'pre-existing output must survive packaging');
const invalid = spawnSync(
  process.execPath,
  ['scripts/package-portable.mjs', '--version', '../../escape', '--out', output],
  { windowsHide: true, encoding: 'utf8' },
);
assert.notEqual(invalid.status, 0);
execFileSync(
  process.execPath,
  ['scripts/package-portable.mjs', '--exe', candidate, '--out', output],
  { windowsHide: true, stdio: 'pipe' },
);
assert.equal(
  await readFile(join(preserved, 'sentinel.txt'), 'utf8'),
  'pre-existing output must survive packaging',
);
assert(!(await readdir(output)).some((item) => item.startsWith('.prism-portable-')));
const zip = join(output, `${name}.zip`);
assert(
  (await readFile(`${zip}.sha256`, 'utf8')).startsWith(
    createHash('sha256')
      .update(await readFile(zip))
      .digest('hex'),
  ),
);
const unpacked = join(directory, 'unpacked');
execFileSync(
  'powershell.exe',
  [
    '-NoProfile',
    '-NonInteractive',
    '-Command',
    'Add-Type -AssemblyName System.IO.Compression.FileSystem\n[System.IO.Compression.ZipFile]::ExtractToDirectory($env:PRISM_TEST_ZIP, $env:PRISM_TEST_OUT)',
  ],
  { windowsHide: true, env: { ...process.env, PRISM_TEST_ZIP: zip, PRISM_TEST_OUT: unpacked } },
);
let appRoot = join(unpacked, name);
assert.deepEqual(
  (await readdir(appRoot)).sort(),
  ['README-portable.txt', 'prism.exe', 'prism.portable', 'notices'].sort(),
);
assert.equal(await hashFile(join(appRoot, 'prism.exe')), candidateSha256);
assert.deepEqual((await readdir(join(appRoot, 'notices'))).sort(), [...noticeNames].sort());
for (const notice of notices)
  assert.deepEqual(await readFile(join(appRoot, 'notices', notice.name)), notice.bytes);
let data = join(appRoot, 'PrismData');
const source = join(directory, 'source');
await mkdir(source);
await writeFile(
  join(source, '출장.md'),
  '# 제주 출장\n제주 출장 경비 항공료 120000원과 숙박비 80000원.',
);
const writeSettings = async (folder, shortcut) => {
  await mkdir(join(folder, 'PrismData'), { recursive: true });
  await writeFile(
    join(folder, 'PrismData', 'settings.json'),
    JSON.stringify({
      onboardingVersion: 1,
      shortcut,
      startOnLogin: true,
      clipboard: { enabled: false, onboarded: true },
      updates: { autoCheck: false },
      scopes: [
        {
          id: 'source',
          name: '합성 자료',
          path: source,
          filenameEnabled: true,
          contentEnabled: true,
        },
        {
          id: 'app',
          name: '포터블 실행 폴더',
          path: folder,
          filenameEnabled: false,
          contentEnabled: true,
        },
      ],
    }),
  );
  await writeFile(join(folder, 'PrismData', 'exclude-me.md'), 'portableprivatecanary');
};
await writeSettings(appRoot, 'Ctrl+Alt+Shift+F12');
const clipboardText = 'synthetic portable clipboard snapshot';
const clipboardId = createHash('sha256').update('text\0').update(clipboardText).digest('hex');
await mkdir(join(data, 'clipboard', 'blobs'), { recursive: true });
await writeFile(join(data, 'clipboard', 'blobs', `${clipboardId}.txt`), clipboardText);
await writeFile(
  join(data, 'clipboard', 'index.json'),
  JSON.stringify({
    version: 1,
    items: [
      {
        id: clipboardId,
        hash: clipboardId,
        kind: 'text',
        preview: clipboardText,
        byteSize: Buffer.byteLength(clipboardText),
        createdMs: Date.now(),
        lastCopiedMs: Date.now(),
        copyCount: 1,
        pinned: false,
      },
    ],
  }),
);
console.log('[portable] package and checksum verified');
let instance, other, secondary;
const env = () => {
  const value = { ...process.env, PRISM_DEV_TEST_MODE: '1' };
  delete value.PRISM_DEV_DATA_DIR;
  delete value.WEBVIEW2_USER_DATA_FOLDER;
  delete value.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS;
  return value;
};
const credential = { slot: 'external', endpoint: 'https://example.org/v1' };
try {
  instance = await launch(data, { portable: true, executable: join(appRoot, 'prism.exe') });
  let { main, quick, invoke } = instance;
  const runtime = await invoke('runtime_info');
  assert.equal(runtime.installKind, 'portable');
  assert.equal(runtime.development, profile === '0');
  assert.equal((await invoke('get_settings')).startOnLogin, false);
  await main.getByRole('button', { name: '설정', exact: true }).click();
  assert.equal(await main.getByLabel('로그인 시 자동 실행', { exact: true }).isDisabled(), true);
  assert((await main.getByRole('dialog').innerText()).includes('포터블에서는 로그인'));
  await main.getByLabel('로그인 시 자동 실행', { exact: true }).scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(evidence, 'portable-settings.png') });
  await main.getByRole('button', { name: '취소', exact: true }).click();
  const settings = await invoke('get_settings');
  const rejected = await main.evaluate(async (settings) => {
    try {
      await window.__TAURI_INTERNALS__.invoke('save_settings', {
        settings: { ...settings, startOnLogin: true },
        expectedRevision: settings.revision,
      });
      return false;
    } catch {
      return true;
    }
  }, settings);
  assert.equal(rejected, true);
  assert.equal((await invoke('get_settings')).revision, settings.revision);
  await invoke('save_settings', {
    settings: { ...settings, theme: 'dark' },
    expectedRevision: settings.revision,
  });
  await main.evaluate(() =>
    localStorage.setItem(
      'prism.draft.v1',
      JSON.stringify({ title: '이동해도 남는 메모', body: '# 한글과 🙂 메모' }),
    ),
  );
  assert((await quick.evaluate(() => localStorage.getItem('prism.draft.v1'))).includes('이동해도'));
  const conversation = await invoke('rag_create', { title: '포터블 대화' });
  const document = await invoke('doc_create', {
    input: {
      title: '포터블 문서',
      purpose: '제주 출장',
      scopeIds: ['source'],
      templateMarkdown: null,
    },
  });
  await invoke('generation_set_key', { ...credential, key: 'synthetic-portable-key' });
  await until(async () => {
    const state = await invoke('content_status');
    return state.phase === 'ready' && state.indexed === 2 + noticeNames.length;
  }, 'portable sources exclude all managed data');
  const hidden = await invoke('search_content', {
    query: 'portableprivatecanary',
    extension: '',
    scopeId: null,
    requestId: crypto.randomUUID(),
  });
  assert.equal(hidden.hits.length, 0);
  for (const dir of [
    'clipboard',
    'content',
    'dictionary',
    'embedding',
    'rag',
    'documents',
    'generation-secrets',
    'WebView2',
  ])
    assert((await stat(join(data, dir))).isDirectory(), `${dir} stays in PrismData`);

  // The actual single-instance plugin must hand off only within this portable copy.
  secondary = spawn(join(appRoot, 'prism.exe'), [], {
    env: env(),
    windowsHide: true,
    stdio: 'ignore',
  });
  await until(() => secondary.exitCode !== null, 'same-copy single-instance handoff');
  assert.equal(secondary.exitCode, 0);
  secondary = null;
  const otherRoot = join(directory, 'other-copy');
  await mkdir(otherRoot);
  await copyFile(join(appRoot, 'prism.exe'), join(otherRoot, 'prism.exe'));
  writePackageNotices(otherRoot, notices);
  await writeFile(join(otherRoot, 'prism.portable'), '');
  await writeSettings(otherRoot, 'Ctrl+Alt+Shift+F1');
  other = await launch(join(otherRoot, 'PrismData'), {
    portable: true,
    executable: join(otherRoot, 'prism.exe'),
  });
  assert.equal((await other.invoke('runtime_info')).installKind, 'portable');
  assert.equal((await other.invoke('rag_list', { query: '', limit: 100 })).length, 0);
  assert.equal(await other.main.evaluate(() => localStorage.getItem('prism.draft.v1')), null);
  assert.deepEqual(other.errors, []);
  await shutdown(other);
  other = null;

  await invoke('model_import', {
    path: resolve('.dev/models/embeddinggemma-2-Q8_0.gguf'),
    runtime: false,
  });
  await invoke('model_import', {
    path: resolve('.dev/runtime/llama-b11960-bin-win-cpu-x64.zip'),
    runtime: true,
  });
  const semantic = async () => {
    await instance.invoke('model_start');
    await until(
      async () => {
        const state = await instance.invoke('content_status');
        if (state.embeddingError)
          throw new Error(`Portable embedding failed: ${state.embeddingError}`);
        return state.phase === 'ready' && state.embedded >= 1;
      },
      'actual portable Lance vectors ready',
      120000,
    );
    const response = await instance.invoke('search_semantic', {
      query: '제주도 출장 경비',
      extension: '',
      scopeId: 'source',
      requestId: crypto.randomUUID(),
    });
    assert(response.hits[0].chunk.text.includes('120000원'));
  };
  console.log('[portable] verifying actual model and Lance before moving');
  await semantic();
  await invoke('model_stop');
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;

  const moved = join(directory, '이동한 포터블');
  // Both resolved endpoints stay under this test's explicitly named temporary directory.
  for (const path of [appRoot, moved]) {
    const child = relative(resolve(directory), resolve(path));
    assert(child && child !== '..' && !child.startsWith(`..${sep}`) && !isAbsolute(child));
  }
  await until(async () => {
    try {
      await rename(appRoot, moved);
      return true;
    } catch (error) {
      if (!['EPERM', 'EACCES', 'EBUSY'].includes(error.code)) throw error;
      return false;
    }
  }, 'owned portable directory moves after shutdown');
  appRoot = moved;
  data = join(appRoot, 'PrismData');
  console.log('[portable] data directory moved; verifying reopened state');
  instance = await launch(data, { portable: true, executable: join(appRoot, 'prism.exe') });
  ({ main, quick, invoke } = instance);
  assert.equal((await invoke('get_settings')).theme, 'dark');
  assert.equal(
    (await invoke('rag_list', { query: '포터블 대화', limit: 100 }))[0].id,
    conversation.id,
  );
  assert.equal(
    (await invoke('doc_get', { id: document.document.id, revisionId: null })).document.id,
    document.document.id,
  );
  assert.equal((await invoke('generation_credential_status', credential)).hasKey, true);
  assert.equal((await invoke('model_status')).modelReady, true);
  assert.equal((await invoke('model_status')).runtimeReady, true);
  assert.equal((await invoke('model_status')).phase, 'ready');
  assert.equal(await invoke('clipboard_text', { id: clipboardId }), clipboardText);
  await semantic();
  await main.getByRole('button', { name: '자연어', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('제주도 출장 경비');
  await until(
    async () => (await main.getByRole('option').count()) > 0,
    'moved portable semantic UI',
  );
  assert((await main.getByRole('option').first().innerText()).includes('출장.md'));
  await main.screenshot({ path: join(evidence, 'portable-moved-semantic.png') });
  await invoke('model_stop');
  await main.getByRole('button', { name: '문서', exact: true }).click();
  assert.equal(
    await main.getByRole('textbox', { name: 'Markdown', exact: true }).inputValue(),
    '# 한글과 🙂 메모',
  );
  assert((await quick.evaluate(() => localStorage.getItem('prism.draft.v1'))).includes('🙂'));
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;

  // These are debug-only harness behaviors. Production release ignores the
  // data/test overrides and presents native dialogs for startup errors, so an
  // unattended exit assertion would test the wrong contract there.
  if (profile === '0') {
    const override = join(directory, 'debug-override');
    instance = await launch(override, { executable: join(appRoot, 'prism.exe') });
    assert.equal((await instance.invoke('runtime_info')).installKind, 'isolated');
    assert.equal((await instance.invoke('rag_list', { query: '', limit: 100 })).length, 0);
    await shutdown(instance);
    instance = null;
    const badRoot = join(directory, 'unwritable-data');
    await mkdir(badRoot);
    await copyFile(join(appRoot, 'prism.exe'), join(badRoot, 'prism.exe'));
    await writeFile(join(badRoot, 'prism.portable'), '');
    await writeFile(join(badRoot, 'PrismData'), 'file blocks the data directory');
    const failed = spawnSync(join(badRoot, 'prism.exe'), [], {
      env: env(),
      windowsHide: true,
      encoding: 'utf8',
      timeout: 25000,
    });
    assert.equal(
      failed.error,
      undefined,
      'Startup errors must exit rather than wait for a test dialog',
    );
    assert.notEqual(failed.status, 0);
    assert(failed.stderr.includes('Prism data location failed'));
    assert.equal(
      await readFile(join(badRoot, 'PrismData'), 'utf8'),
      'file blocks the data directory',
    );
    assert.deepEqual(
      (await readdir(badRoot)).sort(),
      ['PrismData', 'prism.exe', 'prism.portable'].sort(),
    );
    const environmentRoot = join(directory, 'blocked-webview-override');
    await mkdir(environmentRoot);
    await copyFile(join(appRoot, 'prism.exe'), join(environmentRoot, 'prism.exe'));
    await writeFile(join(environmentRoot, 'prism.portable'), '');
    const externalProfile = join(directory, 'external-webview');
    const refused = spawnSync(join(environmentRoot, 'prism.exe'), [], {
      env: { ...env(), WEBVIEW2_USER_DATA_FOLDER: externalProfile },
      windowsHide: true,
      encoding: 'utf8',
      timeout: 25000,
    });
    assert.equal(refused.error, undefined);
    assert.notEqual(refused.status, 0);
    assert(refused.stderr.includes('WEBVIEW2_USER_DATA_FOLDER'));
    await assert.rejects(stat(externalProfile), { code: 'ENOENT' });
  }
  assert.equal(await hashFile(candidate), candidateSha256);
  assert.equal(await hashFile(join(appRoot, 'prism.exe')), candidateSha256);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      candidateSha256,
      release: profile === '1',
      model: 'actual pinned EmbeddingGemma 2',
      movedData: true,
      clipboard: profile === '0' ? 'memory' : 'disabled with seeded local history',
      debugStartupCases: profile === '0',
      externalAiRequests: 0,
    }),
  );
} finally {
  if (secondary && secondary.exitCode === null) secondary.kill();
  if (other) await shutdown(other);
  if (instance) await shutdown(instance);
}
