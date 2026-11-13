// Real settings/blob/index writes in a private portable debug app. No OS clipboard.
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash, randomBytes, randomInt } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32', 'This bundled WebView2 regression is Windows-only');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply the known bundled debug executable');
const expectedSha = process.env.PRISM_TEST_APP_SHA256;
assert.match(expectedSha ?? '', /^[a-f0-9]{64}$/, 'Supply its expected SHA-256');
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
async function hashFile(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
const sourceExecutable = resolve(process.env.PRISM_TEST_EXECUTABLE);
assert.equal(await hashFile(sourceExecutable), expectedSha, 'Executable must match the build');
const directory = await mkdtemp(join(tmpdir(), 'prism-persistence-'));
const executable = join(directory, 'prism.exe');
const dataDir = join(directory, 'PrismData');
const evidence = join(directory, 'evidence');
await mkdir(dataDir);
await mkdir(evidence);
await copyFile(sourceExecutable, executable);
assert.equal(await hashFile(executable), expectedSha);
await writeFile(join(directory, 'prism.portable'), '');
const nonce = randomBytes(16).toString('hex');
const initialRevision = randomInt(1, 0x7fffffff);
await writeFile(
  join(dataDir, 'settings.json'),
  JSON.stringify({
    schemaVersion: 1,
    revision: initialRevision,
    onboardingVersion: 1,
    theme: 'light',
    language: 'ko',
    shortcut: 'Ctrl+Alt+Shift+F10',
    startOnLogin: false,
    scopes: [],
    clipboard: { enabled: false, onboarded: true, captureImages: true },
    updates: { autoCheck: false },
    ocr: { enabled: false },
    generation: { local: null, external: null },
  }),
);
const storeDir = join(dataDir, 'clipboard');
const snapshot = async () => JSON.parse(await readFile(join(storeDir, 'index.json'), 'utf8'));
const diskSettings = async () => JSON.parse(await readFile(join(dataDir, 'settings.json'), 'utf8'));
const result = {
  passed: false,
  executableSha256: expectedSha,
  sourceCommit: process.env.PRISM_TEST_SOURCE_SHA ?? null,
  directory,
  browser: 'Browser plugin not available; repository Playwright/CDP harness',
  clipboard: 'synthetic MemoryClipboard; production store writer and PNG encoder',
  systemClipboardUsed: false,
  checks: [],
  screenshots: [],
  warnings: [],
};
let instance;
let verified = false;
let stage = 'launch';

// Prove this endpoint is owned by the exact spawned child before the first IPC.
async function proveEndpointOwner(childPid, port) {
  assert(Number.isInteger(childPid) && childPid > 0);
  assert(Number.isInteger(port) && port > 1024 && port < 65536);
  const script = `
    $ErrorActionPreference = 'Stop'
    $owners = @(Get-NetTCPConnection -State Listen -LocalPort ${port} | Select-Object -ExpandProperty OwningProcess -Unique)
    if ($owners.Count -ne 1) { throw 'Endpoint owner is ambiguous' }
    $cursorPid = [int]$owners[0]
    $chain = @()
    for ($depth = 0; $depth -lt 32; $depth++) {
      $chain += $cursorPid
      if ($cursorPid -eq ${childPid}) { $chain | ConvertTo-Json -Compress; exit 0 }
      $process = Get-CimInstance Win32_Process -Filter "ProcessId=$cursorPid"
      if (!$process -or !$process.ParentProcessId) { break }
      $cursorPid = [int]$process.ParentProcessId
    }
    throw 'Endpoint is not a descendant of the owned app'
  `;
  const { stdout } = await promisify(execFile)(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    { windowsHide: true, timeout: 20_000 },
  );
  return JSON.parse(stdout);
}

async function start(revision) {
  verified = false;
  instance = await launch(dataDir, { portable: true, executable, readyHeading: false });
  const ownerChain = await proveEndpointOwner(instance.child.pid, instance.port);
  const runtime = await instance.invoke('runtime_info');
  assert.equal(runtime.native, true);
  assert.equal(runtime.platform, 'windows');
  assert.equal(runtime.development, true, 'Synthetic capture requires debug assertions');
  assert.equal(runtime.installKind, 'portable');
  assert.equal((await instance.invoke('get_settings')).revision, revision, 'Private profile nonce');
  assert.equal((await instance.invoke('paste_fixture')).clipboard, null, 'Memory backend required');
  verified = true;
  result.checks.push({ name: 'owned portable debug profile', runtime, ownerChain, revision });
  for (const page of [instance.main, instance.quick])
    page.on('console', (message) => {
      if (message.type() === 'warning') result.warnings.push(message.text());
    });
  await instance.main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
  await instance.main.getByRole('button', { name: '클립보드 기록', exact: true }).click();
}

async function saveRetention(enabled) {
  const { main, invoke } = instance;
  const before = (await invoke('get_settings')).revision;
  await main.getByRole('button', { name: '보관 설정', exact: true }).click();
  await main.getByLabel('클립보드 기록 수집', { exact: true }).setChecked(enabled);
  await main.getByLabel('최대 항목 수', { exact: true }).fill('40');
  await main.getByLabel('보관 일수 (0: 무기한)', { exact: true }).fill('0');
  await main.getByLabel('최대 저장 공간 (MiB)', { exact: true }).fill('32');
  await main.getByRole('button', { name: '설정 저장', exact: true }).click();
  await until(async () => (await invoke('get_settings')).revision > before, 'settings saved');
  const saved = await diskSettings();
  assert.equal(saved.clipboard.enabled, enabled);
  assert.equal(saved.clipboard.maxItems, 40);
  assert.equal(saved.clipboard.maxAgeDays, 0);
  assert.equal(saved.clipboard.maxBytes, 32 * 1024 ** 2);
  assert.equal(saved.startOnLogin, false);
  assert.deepEqual(saved.scopes, []);
  assert.deepEqual(saved, await invoke('get_settings'), 'Disk matches returned settings');
  result.checks.push({ name: 'UI settings replacement', revision: saved.revision, enabled });
  return saved;
}

async function screenshot(name, width, height) {
  const { main } = instance;
  await main.setViewportSize({ width, height });
  await main.getByRole('button', { name: '보관 설정', exact: true }).waitFor();
  await until(
    async () => (await main.getByRole('option').count()) === 2,
    'screenshot history ready',
  );
  await main.locator('.clipboard-full-text').filter({ hasText: nonce }).waitFor();
  assert.equal(await main.title(), 'Prism');
  assert.match(main.url(), /^http:\/\/tauri\.localhost\//);
  assert.equal(await main.locator('vite-error-overlay').count(), 0);
  assert(await main.locator('body').innerText(), 'Meaningful rendered content');
  assert(
    await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
    'No horizontal document overflow',
  );
  const path = join(evidence, `${name}.png`);
  await main.screenshot({ path, animations: 'disabled' });
  result.screenshots.push({ path, url: main.url(), title: await main.title(), width, height });
}

async function stop() {
  if (!instance) return;
  const current = instance;
  const ownedProfile = verified;
  instance = null;
  verified = false;
  result.errors = [...(result.errors ?? []), ...current.errors];
  if (ownedProfile) {
    await shutdown(current);
    assert.equal(current.child.exitCode, 0, 'Owned app exits cleanly');
  } else {
    await current.browser.close().catch(() => {});
    current.child.kill();
  }
}

try {
  await start(initialRevision);
  stage = 'bounded fixture inputs';
  await assert.rejects(
    () => instance.invoke('clipboard_capture_fixture', { kind: 'text', nonce: 'invalid' }),
    /Invalid synthetic clipboard nonce/,
  );
  await assert.rejects(
    () => instance.invoke('clipboard_capture_fixture', { kind: 'unsupported', nonce }),
    /Unknown synthetic clipboard kind/,
  );
  assert.equal((await instance.invoke('clipboard_status')).count, 0);
  stage = 'enable and save settings';
  await saveRetention(true);
  stage = 'produce new text and image through app writer';
  await instance.invoke('clipboard_capture_fixture', { kind: 'text', nonce });
  await instance.invoke('clipboard_capture_fixture', { kind: 'image', nonce });
  const text = `Prism 저장 회귀 ${nonce} 한글 🙂`;
  const textId = hash(Buffer.concat([Buffer.from('text\0'), Buffer.from(text)]));
  const width = 96,
    height = 64;
  const rgba = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++)
      rgba.set([26 + x, 94 + y, 67 + Math.floor(x / 2), 255], (y * width + x) * 4);
  const dimensions = Buffer.alloc(8);
  dimensions.writeUInt32LE(width, 0);
  dimensions.writeUInt32LE(height, 4);
  const imageId = hash(Buffer.concat([Buffer.from('image\0'), dimensions, rgba]));
  const index = await snapshot();
  assert.equal(index.items.length, 2);
  assert.deepEqual(new Set(index.items.map((item) => item.id)), new Set([textId, imageId]));
  assert.equal(await readFile(join(storeDir, 'blobs', `${textId}.txt`), 'utf8'), text);
  const png = await readFile(join(storeDir, 'blobs', `${imageId}.png`));
  const decoded = await instance.main.evaluate(
    async (data) => {
      const image = new Image();
      image.src = data;
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const context = canvas.getContext('2d');
      context.drawImage(image, 0, 0);
      return {
        width: canvas.width,
        height: canvas.height,
        rgba: Array.from(context.getImageData(0, 0, canvas.width, canvas.height).data),
      };
    },
    `data:image/png;base64,${png.toString('base64')}`,
  );
  assert.equal(decoded.width, width);
  assert.equal(decoded.height, height);
  assert.deepEqual(
    Buffer.from(decoded.rgba),
    rgba,
    'Independent browser decoder verifies all pixels',
  );
  result.checks.push({ name: 'new app text/PNG blobs', textId, imageId, pixels: width * height });
  stage = 'UI pin and restore to memory';
  const { main, invoke } = instance;
  await until(async () => (await main.getByRole('option').count()) === 2, 'new items rendered');
  await main.getByRole('option').filter({ hasText: text }).click();
  await main.getByRole('button', { name: '고정', exact: true }).click();
  await until(
    async () => (await snapshot()).items.find((item) => item.id === textId).pinned,
    'pin',
  );
  await main.getByRole('button', { name: '복사', exact: true }).click();
  await until(async () => (await invoke('paste_fixture')).clipboard?.text === text, 'text restore');
  await main.getByRole('button', { name: '이미지', exact: true }).click();
  await main.locator('.clipboard-image').waitFor();
  await until(
    () => main.locator('.clipboard-image').evaluate((image) => image.naturalWidth === 96),
    'app thumbnail decoded',
  );
  await main.getByRole('button', { name: '복사', exact: true }).click();
  await until(
    async () => (await invoke('paste_fixture')).clipboard?.kind === 'image',
    'image restore',
  );
  assert.deepEqual((await invoke('paste_fixture')).clipboard, { kind: 'image', width, height });
  for (const item of (await snapshot()).items) assert.equal(item.copyCount, 1, 'No recapture');
  await main.getByRole('button', { name: '전체', exact: true }).click();
  await screenshot('desktop', 1280, 840);
  await screenshot('narrow', 760, 900);
  stage = 'disable collection and restart';
  const saved = await saveRetention(false);
  const savedIndex = await snapshot();
  const blobHashes = {};
  for (const name of await readdir(join(storeDir, 'blobs')))
    blobHashes[name] = hash(await readFile(join(storeDir, 'blobs', name)));
  await stop();
  await start(saved.revision);
  assert.deepEqual(await diskSettings(), saved);
  assert.deepEqual(await snapshot(), savedIndex);
  assert.equal((await instance.invoke('get_settings')).clipboard.enabled, false);
  await until(
    async () => (await instance.main.getByRole('option').count()) === 2,
    'restart history',
  );
  await instance.main.getByRole('option').filter({ hasText: text }).click();
  await instance.main.getByRole('button', { name: '고정 해제', exact: true }).waitFor();
  await until(
    async () => (await instance.main.locator('.clipboard-full-text').innerText()) === text,
    'reopened full text',
  );
  for (const [name, digest] of Object.entries(blobHashes))
    assert.equal(
      hash(await readFile(join(storeDir, 'blobs', name))),
      digest,
      'Blob survived restart',
    );
  await screenshot('restart', 1280, 840);
  assert.equal(
    (await readdir(dataDir)).some((name) => name.startsWith('.prism-write-')),
    false,
  );
  assert.equal(
    (await readdir(storeDir)).some((name) => name.startsWith('.prism-write-')),
    false,
  );
  assert.equal(
    (await readdir(join(storeDir, 'blobs'))).some((name) => name.startsWith('.prism-write-')),
    false,
  );
  result.checks.push({
    name: 'restart settings/index/blob/pin equality',
    revision: saved.revision,
    blobHashes,
  });
  await stop();
  assert.deepEqual(result.errors, []);
  assert.deepEqual(result.warnings, []);
  result.passed = true;
} catch (error) {
  result.failure = { stage, message: String(error.message), stack: error.stack };
  if (verified)
    await instance.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
  process.exitCode = 1;
} finally {
  try {
    await stop();
  } catch (error) {
    result.passed = false;
    result.cleanupFailure = String(error.message);
    process.exitCode = 1;
  }
  await writeFile(join(directory, 'result.json'), `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify(result));
}
