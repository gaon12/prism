// Actual DOCX/HWPX reader/writer and bundled UI. Only native file-picker
// responses are supplied. A real save response is held without changing data.
// Browser plugin not available: repository Playwright/WebView2 harness.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { loadPackageNotices, writePackageNotices } from '../../scripts/package-notices.mjs';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply an explicit candidate executable');
const profile = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(profile));
const root = await mkdtemp(join(tmpdir(), 'prism-form-input-'));
const portable = join(root, 'portable');
const data = join(portable, 'PrismData');
await mkdir(data, { recursive: true });
const original = resolve(process.env.PRISM_TEST_EXECUTABLE);
const executable = join(portable, 'prism.exe');
await copyFile(original, executable);
writePackageNotices(portable, loadPackageNotices(original));
await writeFile(join(portable, 'prism.portable'), '');
const hash = async (path) => {
  const digest = createHash('sha256');
  for await (const bytes of createReadStream(path)) digest.update(bytes);
  return digest.digest('hex');
};
const executableSha256 = await hash(executable);
assert.equal(await hash(original), executableSha256);
const python = process.env.PRISM_TEST_PYTHON ?? 'python';
const createForm = (path, format, changed = false, extraNames = []) => {
  const lines = [
    ...(changed ? ['참조: ___'] : []),
    '이름: {{이름}}',
    '주소: ___',
    '날짜: ___',
    ...extraNames.map((key) => `${key}: {{${key}}}`),
  ];
  const xml =
    format === 'docx'
      ? '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>' +
        lines.map((text) => `<w:p><w:r><w:t>${text}</w:t></w:r></w:p>`).join('') +
        '</w:body></w:document>'
      : '<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">' +
        lines.map((text) => `<hp:p><hp:run><hp:t>${text}</hp:t></hp:run></hp:p>`).join('') +
        '</hs:sec>';
  const parts =
    format === 'docx'
      ? { 'word/document.xml': xml, 'word/styles.xml': '<styles>KEEP_STYLE_123</styles>' }
      : {
          mimetype: 'application/hwp+zip',
          'Contents/section0.xml': xml,
          'Contents/header.xml': '<header>KEEP_STYLE_123</header>',
        };
  execFileSync(
    python,
    [
      '-c',
      'import json,sys,zipfile\nwith zipfile.ZipFile(sys.argv[1],"w") as z:\n for name,text in json.loads(sys.argv[2]).items():\n  z.writestr(name,text,compress_type=zipfile.ZIP_STORED if name=="mimetype" else zipfile.ZIP_DEFLATED)',
      path,
      JSON.stringify(parts),
    ],
    { windowsHide: true },
  );
};
const inspect = (path) =>
  JSON.parse(
    execFileSync(
      python,
      [
        '-c',
        'import json,sys,zipfile\nwith zipfile.ZipFile(sys.argv[1]) as z:\n print(json.dumps({n:z.read(n).decode("utf-8") for n in z.namelist()},ensure_ascii=True))',
        path,
      ],
      { windowsHide: true, encoding: 'utf8' },
    ),
  );
const docx = join(root, '기존 양식.docx');
const hwpx = join(root, '기존 양식.hwpx');
createForm(docx, 'docx');
createForm(hwpx, 'hwpx');
const docxSha256 = await hash(docx);
const hwpxSha256 = await hash(hwpx);
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    startOnLogin: false,
    shortcut: 'Ctrl+Alt+Shift+F9',
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: false },
    scopes: [],
  }),
);
let instance;
try {
  instance = await launch(data, { executable, portable: true });
  const { main, invoke } = instance;
  assert.equal((await invoke('runtime_info')).development, profile === '0');
  await main.evaluate((source) => {
    const original = window.fetch.bind(window);
    window.__formInput = { source, destination: null, hold: false, writes: 0, release: null };
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (url.hostname !== 'ipc.localhost') return original(input, options);
      const command = decodeURIComponent(url.pathname.slice(1));
      const test = window.__formInput;
      if (command === 'plugin:dialog|open' || command === 'plugin:dialog|save')
        return new Response(
          JSON.stringify(command.endsWith('|open') ? test.source : test.destination),
          {
            headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
          },
        );
      const response = await original(input, options);
      if (command === 'form_fill') {
        test.writes++;
        if (test.hold)
          await new Promise((resolve) => {
            test.release = resolve;
          });
      }
      return response;
    };
  }, docx);
  await main.getByRole('button', { name: '문서', exact: true }).click();
  const fillMode = main.getByRole('button', { name: '기존 양식 채우기', exact: true });
  const newMode = main.getByRole('button', { name: '새 문서 작성', exact: true });
  await fillMode.click();
  await main.getByRole('button', { name: '양식 파일 열기', exact: true }).click();
  const fields = main.locator('.form-fill-fields textarea');
  await until(async () => (await fields.count()) === 3, 'actual DOCX fields');
  await fields.nth(0).fill('홍길동');
  await fields.nth(1).fill('서울');
  await fields.nth(2).fill('2026-11-07');
  await main.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  assert(await newMode.isDisabled(), 'dirty form blocks destructive mode switch');
  assert(await main.getByRole('button', { name: '다른 양식', exact: true }).isDisabled());
  await main.getByRole('button', { name: '검색', exact: true }).click();
  assert.equal(
    await fields.nth(0).inputValue(),
    '홍길동',
    'tab navigation retains unsaved form input',
  );
  const save = main.getByRole('button', { name: '채워서 새 파일로 저장', exact: true });
  await save.click();
  await until(() => save.isEnabled(), 'save picker cancellation returns control');
  assert(await newMode.isDisabled(), 'cancelled picker does not mark input saved');
  assert.equal(await main.evaluate(() => window.__formInput.writes), 0);
  const first = join(root, '첫 저장.docx');
  await main.evaluate((path) => {
    window.__formInput.destination = path;
    window.__formInput.hold = true;
  }, first);
  await save.click();
  await until(
    () => main.evaluate(() => Boolean(window.__formInput.release)),
    'real native save response held',
  );
  await fields.nth(0).fill('나중에 입력한 이름');
  const written = inspect(first);
  assert(written['word/document.xml'].includes('홍길동'));
  assert(!written['word/document.xml'].includes('나중에 입력한 이름'));
  assert.equal(written['word/styles.xml'], inspect(docx)['word/styles.xml']);
  await main.evaluate(() => {
    window.__formInput.hold = false;
    window.__formInput.release();
  });
  await main.getByText(first, { exact: false }).waitFor();
  assert.equal(await fields.nth(0).inputValue(), '나중에 입력한 이름');
  assert(await newMode.isDisabled(), 'only the written input snapshot becomes clean');
  await main.getByRole('button', { name: '입력 변경 버리기', exact: true }).click();
  await until(() => newMode.isEnabled(), 'discard restores the actually saved inputs');
  assert.equal(await fields.nth(0).inputValue(), '홍길동');
  assert.equal(await hash(docx), docxSha256);
  await newMode.click();
  await main.locator('.doc-sidebar').getByRole('button', { name: '새 문서', exact: true }).click();
  await main.getByLabel('문서 제목', { exact: true }).fill('아직 만들지 않은 문서');
  assert(await fillMode.isDisabled(), 'new-document input also blocks destructive mode switch');
  await main.locator('.doc-create').getByRole('button', { name: '취소', exact: true }).click();
  await fillMode.click();
  await main.getByRole('button', { name: '양식 파일 열기', exact: true }).click();
  await until(async () => (await fields.count()) === 3, 'reopened original form');
  await fields.nth(0).fill('새 이름');
  await fields.nth(1).fill('부산');
  await fields.nth(2).fill('2027-01-02');
  createForm(docx, 'docx', true); // Intentional fixture change, not an app write.
  const changedSha256 = await hash(docx);
  const second = join(root, '변경 후 저장.docx');
  await writeFile(second, 'KEEP_EXISTING_DESTINATION');
  await main.evaluate((path) => {
    window.__formInput.destination = path;
  }, second);
  await save.click();
  const reload = main.getByRole('button', { name: '다시 열기', exact: true });
  await reload.waitFor();
  assert.equal(await readFile(second, 'utf8'), 'KEEP_EXISTING_DESTINATION');
  await reload.click();
  const recovery = main.getByRole('region', { name: '이전 양식의 입력', exact: true });
  await recovery.waitFor();
  assert.equal(await fields.count(), 4);
  assert.equal(
    await fields.nth(1).inputValue(),
    '새 이름',
    'stable named placeholder keeps its input',
  );
  for (const index of [0, 2, 3])
    assert.equal(
      await fields.nth(index).inputValue(),
      '',
      'positional blanks are not silently reassigned',
    );
  assert.deepEqual(
    await recovery.locator('textarea').evaluateAll((items) => items.map((item) => item.value)),
    ['부산', '2027-01-02'],
  );
  assert(await save.isDisabled());
  assert(await newMode.isDisabled());
  if (await main.locator('.toast button').count()) await main.locator('.toast button').click();
  await recovery.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(root, 'form-recovery-ko.png'), animations: 'disabled' });
  const settings = await invoke('get_settings');
  await invoke('save_settings', {
    settings: { ...settings, theme: 'dark' },
    expectedRevision: settings.revision,
  });
  await main.setViewportSize({ width: 390, height: 760 });
  assert(await main.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await recovery.scrollIntoViewIfNeeded();
  await main.screenshot({ path: join(root, 'form-recovery-narrow.png'), animations: 'disabled' });
  await main.setViewportSize({ width: 1600, height: 1050 });
  await fields.nth(0).fill('REF-1');
  await fields.nth(2).fill('부산');
  await fields.nth(3).fill('2027-01-02');
  await main.getByRole('button', { name: '이전 입력 검토 완료', exact: true }).click();
  await save.click();
  await until(() => newMode.isEnabled(), 'reviewed changed form saved');
  const changed = inspect(second);
  for (const text of ['참조: REF-1', '이름: 새 이름', '주소: 부산', '날짜: 2027-01-02'])
    assert(changed['word/document.xml'].includes(text));
  assert.equal(changed['word/styles.xml'], inspect(docx)['word/styles.xml']);
  assert.equal(await hash(docx), changedSha256);
  await main.evaluate((path) => {
    window.__formInput.source = path;
  }, hwpx);
  await main.getByRole('button', { name: '다른 양식', exact: true }).click();
  await until(async () => (await fields.count()) === 3, 'actual HWPX fields');
  await fields.nth(0).fill('한글 양식 입력');
  assert(await newMode.isDisabled());
  const outputHwpx = join(root, '채운 양식.hwpx');
  await main.evaluate((path) => {
    window.__formInput.destination = path;
  }, outputHwpx);
  await save.click();
  await until(() => newMode.isEnabled(), 'actual HWPX save clears dirty state');
  const hwpxResult = inspect(outputHwpx);
  assert(hwpxResult['Contents/section0.xml'].includes('한글 양식 입력'));
  assert.equal(hwpxResult.mimetype, 'application/hwp+zip');
  assert.equal(hwpxResult['Contents/header.xml'], inspect(hwpx)['Contents/header.xml']);
  assert.equal(await hash(hwpx), hwpxSha256);
  const reserved = join(root, '영문 항목.docx');
  const names = ['__proto__', 'constructor', 'toString', 'blank:1', 'field:blank:1'];
  createForm(reserved, 'docx', false, names);
  await main.evaluate((path) => {
    window.__formInput.source = path;
  }, reserved);
  await main.getByRole('button', { name: '다른 양식', exact: true }).click();
  await until(
    async () => (await fields.count()) === 3 + names.length,
    'named fields that also exist on Object.prototype',
  );
  for (const index of names.map((_, position) => 3 + position))
    assert.equal(
      await fields.nth(index).inputValue(),
      '',
      'field defaults contain only user-entered values',
    );
  for (const [index, key] of names.entries()) await fields.nth(3 + index).fill(`값 ${key}`);
  for (const [index, key] of names.entries())
    assert.equal(
      await main
        .locator('.form-fill-fields .form-fill-key')
        .nth(3 + index)
        .textContent(),
      key,
      'original placeholder names remain visible',
    );
  createForm(reserved, 'docx', true, names);
  const reservedSha256 = await hash(reserved);
  const reservedOutput = join(root, '영문 항목 저장.docx');
  await main.evaluate((path) => {
    window.__formInput.destination = path;
  }, reservedOutput);
  await save.click();
  await reload.waitFor();
  await reload.click();
  await until(
    async () => (await fields.count()) === 4 + names.length,
    'reserved named fields reloaded',
  );
  for (const [index, key] of names.entries())
    assert.equal(await fields.nth(4 + index).inputValue(), `값 ${key}`);
  await save.click();
  await until(() => newMode.isEnabled(), 'reserved named values saved');
  const reservedResult = inspect(reservedOutput);
  for (const key of names)
    assert(reservedResult['word/document.xml'].includes(`${key}: 값 ${key}`));
  assert(
    reservedResult['word/document.xml'].includes('참조: ___'),
    'a named blank:1 never fills the positional blank',
  );
  assert.equal(await hash(reserved), reservedSha256);
  await newMode.click();
  await main.getByRole('button', { name: '검색', exact: true }).click();
  await main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
  assert.equal(await hash(original), executableSha256);
  assert.equal((await invoke('model_status')).modelReady, false);
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      root,
      executableSha256,
      release: profile === '1',
      sourceFormats: ['docx', 'hwpx'],
      cancelledPickerPreservesInput: true,
      lateSavePreservesNewInput: true,
      ambiguousBlankValuesRecovered: true,
      originalFilesPreserved: true,
      externalAIRequests: 0,
      errors: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
