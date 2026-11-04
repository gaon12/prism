// Uses only generated fixture files and Prism's injected MemoryClipboard.
// Both configuration and WebView profiles are isolated from the user's running app.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { deflateSync } from 'node:zlib';
import { chromium } from '@playwright/test';

const dataDir = await mkdtemp(join(tmpdir(), 'prism-clipboard-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
const storeDir = join(dataDir, 'clipboard');
await mkdir(join(storeDir, 'blobs'), { recursive: true });
await mkdir(evidence, { recursive: true });
await writeFile(join(dataDir, 'settings.json'), JSON.stringify({ shortcut: 'Ctrl+Alt+Shift+F10' }));
const width = 96,
  height = 64;
const rgba = Buffer.alloc(width * height * 4);
for (let y = 0; y < height; y++)
  for (let x = 0; x < width; x++) {
    const offset = (y * width + x) * 4;
    rgba.set([26 + x, 94 + y, 67 + Math.floor(x / 2), 255], offset);
  }
const png = fixturePng(rgba);
const now = Date.now();
const items = [];
for (const [index, [kind, value]] of [
  ['text', '공문서 검토 메모'],
  ['link', 'https://example.org/prism'],
  ['image', null],
].entries()) {
  const imageSize = Buffer.alloc(8);
  imageSize.writeUInt32LE(width, 0);
  imageSize.writeUInt32LE(height, 4);
  const id = createHash('sha256')
    .update(
      kind === 'image'
        ? Buffer.concat([Buffer.from('image\0'), imageSize, rgba])
        : Buffer.concat([Buffer.from('text\0'), Buffer.from(value)]),
    )
    .digest('hex');
  if (kind === 'image') {
    await writeFile(join(storeDir, 'blobs', `${id}.png`), png);
    await writeFile(join(storeDir, 'blobs', `${id}.thumb.png`), png);
  } else await writeFile(join(storeDir, 'blobs', `${id}.txt`), value);
  items.push({
    id,
    hash: id,
    kind,
    preview: value ?? `${width}×${height}`,
    byteSize: kind === 'image' ? png.length * 2 : Buffer.byteLength(value),
    ...(kind === 'image' ? { width, height } : {}),
    createdMs: now - index * 1000,
    lastCopiedMs: now - index * 1000,
    copyCount: 1,
    pinned: false,
  });
}
await writeFile(join(storeDir, 'index.json'), JSON.stringify({ version: 1, items }));
let instance;
try {
  instance = await launch();
  const { main, quick } = instance;
  const errors = [];
  for (const page of [main, quick]) page.on('pageerror', (error) => errors.push(error.message));
  await main.getByRole('button', { name: '기록 사용', exact: true }).click();
  await until(
    async () =>
      JSON.parse(await readFile(join(dataDir, 'settings.json'), 'utf8')).clipboard.onboarded,
    'first-run notice acknowledgment',
  );
  await main.getByRole('button', { name: '클립보드 기록', exact: true }).click();
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  await until(
    async () => (await main.getByRole('option').count()) === 3,
    'all persisted fixture kinds',
  );
  await main.getByRole('button', { name: '고정', exact: true }).click();
  await until(async () => (await snapshot()).items[0].pinned, 'pin persisted');
  await input.fill('clip: ㄱㅁㅅ');
  await until(
    async () => (await main.getByRole('option').count()) === 1,
    'Korean initial clipboard search',
  );
  await main.getByRole('button', { name: '복사', exact: true }).click();
  await main.getByRole('status').filter({ hasText: '복사했어요' }).waitFor();
  assert.equal(
    (await snapshot()).items.find((item) => item.kind === 'text').copyCount,
    1,
    'restore is not a fresh copy',
  );
  await input.fill('clip:');
  await main.getByRole('button', { name: '이미지', exact: true }).click();
  await until(async () => (await main.getByRole('option').count()) === 1, 'image filter');
  await main.locator('.clipboard-image').waitFor();
  assert.equal(
    await main.locator('.clipboard-image').evaluate((image) => image.naturalWidth),
    width,
  );
  await main.getByRole('button', { name: '복사', exact: true }).click();
  assert.equal((await snapshot()).items.length, 3, 'image restore uses the same content hash');
  await main.getByRole('button', { name: '일시정지', exact: true }).click();
  await main.getByRole('status').filter({ hasText: '기록이 일시정지되었습니다.' }).waitFor();
  await main.getByRole('button', { name: '기록 재개', exact: true }).click();
  await main.getByRole('button', { name: '전체', exact: true }).click();
  await until(
    async () => (await main.getByRole('option').count()) === 3,
    'all history loaded before screenshot',
  );
  await main.screenshot({ path: join(evidence, 'clipboard-main.png') });
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await until(
    async () => (await quick.getByRole('option').count()) === 3,
    'Spotlight shared history',
  );
  const quickInput = quick.getByRole('textbox', { name: '검색어', exact: true });
  const selected = await quickInput.getAttribute('aria-activedescendant');
  await quickInput.press('ArrowDown');
  assert.notEqual(await quickInput.getAttribute('aria-activedescendant'), selected);
  await quick.locator('.clipboard-image').waitFor();
  await quick.screenshot({ path: join(evidence, 'clipboard-quick.png') });
  await quickInput.press('Control+Enter');
  await main.getByRole('button', { name: '고정 외 기록 삭제', exact: true }).click();
  await main
    .locator('.clipboard-confirm')
    .getByRole('button', { name: '삭제', exact: true })
    .click();
  await until(
    async () => (await main.getByRole('option').count()) === 1,
    'clear preserves pinned item',
  );
  await main.getByRole('button', { name: '보관 설정', exact: true }).click();
  await main.getByLabel('클립보드 기록 수집', { exact: true }).uncheck();
  await main.getByLabel('보관 일수 (0: 무기한)', { exact: true }).fill('0');
  await main.getByRole('button', { name: '설정 저장', exact: true }).click();
  await main.getByRole('status').filter({ hasText: '기록이 꺼져 있습니다.' }).waitFor();
  const settings = JSON.parse(await readFile(join(dataDir, 'settings.json'), 'utf8'));
  assert.equal(settings.clipboard.enabled, false);
  assert.equal(settings.clipboard.maxAgeDays, 0);
  assert.deepEqual(errors, []);
  await shutdown(instance);
  instance = null;
  instance = await launch();
  await until(
    async () => (await instance.main.getByRole('option').count()) === 1,
    'restart preserves pinned content',
  );
  await instance.main.getByRole('status').filter({ hasText: '기록이 꺼져 있습니다.' }).waitFor();
  assert.equal(
    await instance.main.locator('.clipboard-notice').count(),
    0,
    'notice remains acknowledged',
  );
  console.log(
    JSON.stringify({
      passed: [
        'isolated source',
        'first-run notice',
        'text/link/image',
        'Korean initials',
        'restore without duplicates',
        'image thumbnail',
        'pause/resume',
        'pin/clear',
        'main/quick keyboard selection',
        'settings and restart',
      ],
      evidence,
      dataDir,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}

async function snapshot() {
  return JSON.parse(await readFile(join(storeDir, 'index.json'), 'utf8'));
}
async function until(condition, label) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (await condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out: ${label}`);
}
async function launch() {
  const server = createServer();
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  const child = spawn(resolve('src-tauri/target/debug/prism.exe'), [], {
    windowsHide: true,
    stdio: 'ignore',
    env: {
      ...process.env,
      PRISM_DEV_DATA_DIR: dataDir,
      WEBVIEW2_USER_DATA_FOLDER: join(dataDir, 'webview'),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
  });
  let browser;
  try {
    await until(async () => {
      if (child.exitCode !== null) throw new Error(`Prism exited: ${child.exitCode}`);
      try {
        return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
      } catch {
        return false;
      }
    }, 'WebView debugging endpoint');
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    let main, quick;
    await until(async () => {
      const pages = browser.contexts().flatMap((context) => context.pages());
      main = pages.find(
        (page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'),
      );
      quick = pages.find(
        (page) => page.url().includes('tauri.localhost') && page.url().includes('#quick'),
      );
      return main && quick;
    }, 'bundled desktop windows');
    await main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
    return { child, browser, main, quick };
  } catch (error) {
    await browser?.close();
    child.kill();
    throw error;
  }
}
async function shutdown({ child, browser, main }) {
  try {
    await main.evaluate(() => window.__TAURI_INTERNALS__.invoke('quit_app'));
  } catch {
    /* window closes during command */
  }
  await browser.close();
  await until(() => child.exitCode !== null, 'test process exit').catch(() => child.kill());
}
function fixturePng(rgba) {
  const size = Buffer.alloc(13);
  size.writeUInt32BE(width, 0);
  size.writeUInt32BE(height, 4);
  size[8] = 8;
  size[9] = 6;
  const chunk = (type, body) => {
    const payload = Buffer.concat([Buffer.from(type), body]);
    let crc = 0xffffffff;
    for (const byte of payload) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
    const header = Buffer.alloc(4);
    header.writeUInt32BE(body.length);
    const footer = Buffer.alloc(4);
    footer.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
    return Buffer.concat([header, payload, footer]);
  };
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', size),
    chunk(
      'IDAT',
      deflateSync(
        Buffer.concat(
          Array.from({ length: height }, (_, y) =>
            Buffer.concat([Buffer.from([0]), rgba.subarray(y * width * 4, (y + 1) * width * 4)]),
          ),
        ),
      ),
    ),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
