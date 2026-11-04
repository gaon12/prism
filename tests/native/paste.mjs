// Real native clipboard/IPC/UI, synthetic windows and keyboard only.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { deflateSync } from 'node:zlib';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-paste-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
const blobs = join(directory, 'clipboard', 'blobs');
await mkdir(blobs, { recursive: true });
await mkdir(evidence, { recursive: true });
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F5',
    onboardingVersion: 1,
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
  }),
);
const text = '원래 앱에 붙여넣을 한글과 🙂';
const rgba = Buffer.from([1, 2, 3, 255]);
const size = Buffer.alloc(8);
size.writeUInt32LE(1);
size.writeUInt32LE(1, 4);
const items = [];
for (const [index, value] of [text, 'https://example.org/prism', null].entries()) {
  const kind = value === null ? 'image' : index === 0 ? 'text' : 'link';
  const id = createHash('sha256')
    .update(value === null ? Buffer.concat([Buffer.from('image\0'), size, rgba]) : `text\0${value}`)
    .digest('hex');
  const bytes = value === null ? fixturePng() : Buffer.from(value);
  await writeFile(join(blobs, `${id}.${value === null ? 'png' : 'txt'}`), bytes);
  if (value === null) await writeFile(join(blobs, `${id}.thumb.png`), bytes);
  items.push({
    id,
    hash: id,
    kind,
    preview: value ?? '1×1',
    byteSize: bytes.length,
    ...(value === null ? { width: 1, height: 1 } : {}),
    createdMs: Date.now() - index,
    lastCopiedMs: Date.now() - index,
    copyCount: 1,
    pinned: false,
  });
}
await writeFile(join(directory, 'clipboard', 'index.json'), JSON.stringify({ version: 1, items }));
let instance;
try {
  instance = await launch(directory);
  let { main, quick, invoke } = instance;
  const scenario = (mode) => invoke('paste_fixture', { mode });
  const snapshot = () => invoke('paste_fixture', { mode: null });
  const visible = (label) => invoke('plugin:window|is_visible', { label });
  const paste = (id = items[0].id) => invoke('clipboard_paste', { id });
  for (const [mode, error] of [
    ['noTarget', 'noTarget'],
    ['targetGone', 'targetGone'],
    ['focusFailed', 'focusFailed'],
    ['keysHeld', 'keysHeld'],
    ['inputBlocked', 'inputBlocked'],
    ['unsupported', 'unsupported'],
  ]) {
    await scenario(mode);
    assert.deepEqual(await paste(), { copied: true, pasted: false, error });
    const state = await snapshot();
    assert.deepEqual(state.clipboard, { kind: 'text', text });
    assert.equal(state.pasteRequests, 0);
    assert.equal(state.inputAttempts, mode === 'inputBlocked' ? 1 : 0);
    assert.equal(await visible('main'), true, `main restored after ${mode}`);
  }
  await scenario('ready');
  assert.deepEqual(await paste('missing-id'), {
    copied: false,
    pasted: false,
    error: 'copyFailed',
  });
  assert.equal((await snapshot()).focusRequests, 0);
  assert.equal(await visible('main'), true);
  await scenario('keysHeld');
  const concurrent = await main.evaluate(
    async (id) =>
      Promise.all([
        window.__TAURI_INTERNALS__.invoke('clipboard_paste', { id }),
        window.__TAURI_INTERNALS__.invoke('clipboard_paste', { id }),
      ]),
    items[0].id,
  );
  assert.deepEqual(concurrent.map((result) => result.error).sort(), ['busy', 'keysHeld']);
  assert.equal((await snapshot()).inputAttempts, 0);

  await main.getByRole('button', { name: '클립보드 기록', exact: true }).click();
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('clip: 원래');
  await until(
    async () => (await main.getByRole('option').count()) === 1,
    'selected clipboard text',
  );
  await main.screenshot({ path: join(evidence, 'paste-main.png') });
  await scenario('ready');
  await main.getByRole('button', { name: '원래 앱에 붙여넣기', exact: true }).click();
  await until(async () => (await snapshot()).pasteRequests === 1, 'main paste action');
  await until(
    async () => !(await visible('main')) && !(await visible('quick')),
    'paste hides Prism',
  );
  await invoke('show_main');
  await scenario('ready');
  await main.getByRole('option').first().dblclick();
  await until(async () => (await snapshot()).pasteRequests === 1, 'row double-click paste');
  await invoke('show_main');
  await scenario('ready');
  await main.getByRole('button', { name: '복사', exact: true }).click();
  await main.getByRole('status').filter({ hasText: '복사했어요' }).waitFor();
  assert.equal((await snapshot()).pasteRequests, 0, 'copy stays available without paste');
  assert.equal(await visible('main'), true);

  await invoke('show_quick');
  await quick.locator('.quick-footer').filter({ hasText: '붙여넣기' }).waitFor();
  await scenario('ready');
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Enter');
  await until(async () => (await snapshot()).pasteRequests === 1, 'Spotlight Enter paste');
  await until(async () => !(await visible('quick')), 'Spotlight hides after success');
  await invoke('show_quick');
  await scenario('focusFailed');
  await quick.getByRole('textbox', { name: '검색어', exact: true }).press('Enter');
  await quick.getByRole('status').filter({ hasText: '원래 창으로 전환하지 못했습니다' }).waitFor();
  assert.equal(await visible('quick'), true, 'failed paste keeps Spotlight available');
  assert.equal((await snapshot()).pasteRequests, 0);
  await quick.screenshot({ path: join(evidence, 'paste-quick-fallback.png') });
  await invoke('show_main');
  for (const item of items.slice(1)) {
    await scenario('ready');
    assert.deepEqual(await paste(item.id), { copied: true, pasted: true, error: null });
    const restored = (await snapshot()).clipboard;
    assert.deepEqual(
      restored,
      item.kind === 'image'
        ? { kind: 'image', width: 1, height: 1 }
        : { kind: 'text', text: item.preview },
    );
    await invoke('show_main');
  }
  assert.equal((await invoke('clipboard_list', { query: '', kind: null, limit: 100 })).total, 3);
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;
  instance = await launch(directory);
  ({ main, quick, invoke } = instance);
  assert.deepEqual(await paste(), { copied: true, pasted: false, error: 'noTarget' });
  assert.equal(
    (await snapshot()).inputAttempts,
    0,
    'native target is not persisted across restart',
  );
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      keyboard: 'synthetic only',
      clipboard: 'memory',
      systemInput: false,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}

function fixturePng() {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(1);
  ihdr.writeUInt32BE(1, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
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
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(Buffer.concat([Buffer.from([0]), rgba]))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
