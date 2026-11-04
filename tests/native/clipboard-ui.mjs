// Generated text history, isolated profile and MemoryClipboard only.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-clipboard-ui-'));
const blobs = join(directory, 'clipboard', 'blobs');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(blobs, { recursive: true });
await mkdir(evidence, { recursive: true });
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F2',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
  }),
);
const items = [];
for (let index = 0; index < 40; index++) {
  const value = index < 39 ? `보관 고정 ${index}` : '정리할 항목';
  const id = createHash('sha256').update('text\0').update(value).digest('hex');
  await writeFile(join(blobs, `${id}.txt`), value);
  items.push({
    id,
    hash: id,
    kind: 'text',
    preview: value,
    byteSize: Buffer.byteLength(value),
    createdMs: Date.now() - index * 1000,
    lastCopiedMs: Date.now() - index * 1000,
    copyCount: 1,
    pinned: index < 39,
  });
}
await writeFile(join(directory, 'clipboard', 'index.json'), JSON.stringify({ version: 1, items }));
let instance;
try {
  instance = await launch(directory);
  const { main, quick, invoke } = instance;
  await main.getByRole('button', { name: '클립보드 기록', exact: true }).click();
  await until(
    async () => (await main.getByRole('option').count()) === 40,
    'history fixture loaded',
  );
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  const clear = main.getByRole('button', { name: '고정 외 기록 삭제', exact: true });
  await input.fill('clip: 고정');
  await until(
    async () => (await main.getByRole('option').count()) === 39,
    'filtered pinned history',
  );
  assert.equal(
    await clear.isDisabled(),
    false,
    'An unpinned item outside this filter can still be cleared',
  );
  await main.getByRole('button', { name: '보관 설정', exact: true }).click();
  const before = await invoke('get_settings');
  const save = main.getByRole('button', { name: '설정 저장', exact: true });
  for (const [label, values, restore] of [
    ['최대 항목 수', ['', '10.5', '10001'], '500'],
    ['보관 일수 (0: 무기한)', ['', '-1', '1.5'], '30'],
    ['최대 저장 공간 (MiB)', ['', '16.5', '8193'], '256'],
  ]) {
    const field = main.getByLabel(label, { exact: true });
    for (const value of values) {
      await field.fill(value);
      assert.equal(await save.isDisabled(), true, `${label}: ${value} must not be submitted`);
      assert.equal(await field.getAttribute('aria-invalid'), 'true');
      assert.equal((await invoke('get_settings')).revision, before.revision);
      if (label === '최대 저장 공간 (MiB)' && value === '16.5')
        await main.screenshot({
          path: join(evidence, 'clipboard-invalid-retention.png'),
          animations: 'disabled',
        });
    }
    await field.fill(restore);
  }
  await main.screenshot({ path: join(evidence, 'clipboard-valid-retention.png') });
  await main.getByLabel('최대 항목 수', { exact: true }).fill('40');
  await main.getByLabel('보관 일수 (0: 무기한)', { exact: true }).fill('0');
  await main.getByLabel('최대 저장 공간 (MiB)', { exact: true }).fill('32');
  await save.click();
  await until(
    async () => (await invoke('get_settings')).revision > before.revision,
    'valid integer retention saved',
  );
  const saved = await invoke('get_settings');
  assert.equal(saved.clipboard.maxItems, 40);
  assert.equal(saved.clipboard.maxAgeDays, 0);
  assert.equal(saved.clipboard.maxBytes, 32 * 1024 ** 2);
  await input.fill('clip:');
  await until(
    async () => (await main.getByRole('option').count()) === 40,
    'all rows visible to controller',
  );
  await input.press('ArrowUp');
  const selectedIsVisible = async (page) =>
    page.locator('.clipboard-list').evaluate((list) => {
      const row = list.querySelector('[aria-selected=true]');
      if (!row) return false;
      const a = list.getBoundingClientRect(),
        b = row.getBoundingClientRect();
      return list.scrollTop > 0 && b.top >= a.top - 1 && b.bottom <= a.bottom + 1;
    });
  await until(() => selectedIsVisible(main), 'main keyboard selection scrolls into view');
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await until(async () => (await quick.getByRole('option').count()) === 40, 'quick shared history');
  const quickInput = quick.getByRole('textbox', { name: '검색어', exact: true });
  await quickInput.press('ArrowUp');
  await until(() => selectedIsVisible(quick), 'quick keyboard selection scrolls into view');
  await quick.screenshot({ path: join(evidence, 'clipboard-keyboard-scrolled.png') });
  await quickInput.press('Control+Enter');
  await clear.click();
  await main
    .locator('.clipboard-confirm')
    .getByRole('button', { name: '삭제', exact: true })
    .click();
  await until(
    async () => (await main.getByRole('option').count()) === 39,
    'only pinned items remain',
  );
  assert.equal(await clear.isDisabled(), true);
  await input.fill('clip: 고정 1');
  await until(async () => await clear.isDisabled(), 'filtered pinned-only store cannot clear');
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({ passed: true, directory, clipboard: 'in-memory', systemInput: false }),
  );
} finally {
  if (instance) await shutdown(instance);
}
