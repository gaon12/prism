import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { copyFile, mkdtemp, readFile, readdir, rmdir, unlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { hasReservationAbsenceStatement } from '../native/support/reservation-note.mjs';

const facts = '예산 총액: 347000원, 참가 인원: 7명, 장소: 부산 해운대, 개최일: 2026년 12월 5일.\n';
const validNotes = [
  '예약 번호는 제공된 자료에 없습니다. [S6]',
  '예약 번호는 정해지지 않았으며 이 자료에 기재되어 있지 않다.',
  '| 예약 번호 | 자료에 없음 |',
  '예약 번호는 미정입니다.',
  '예약 번호는 자료에 없으므로 추후 확인이 필요합니다.',
  '예약 번호가 자료에 없다고 명시되어 있습니다.',
];
const echoedInstructions = [
  '예약 번호가 원문에 없으면 자료에 없다고 명시해야 합니다.',
  '예약 번호가 자료에 없다고 답하세요.',
  '예약 번호가 미정이면 미정이라고 안내하세요.',
  '예약 번호가 자료에 기재되어 있지 않으면 없다고 말하세요.',
  '예약 번호를 확인했습니다.\n다른 준비물 없음.',
];

test('actual statements of absence still satisfy the original Korean fixture', () => {
  for (const note of validNotes) assert.equal(hasReservationAbsenceStatement(note), true, note);
});

test('conditional instructions and unrelated absence cannot substitute for an answer', () => {
  for (const note of echoedInstructions)
    assert.equal(hasReservationAbsenceStatement(note), false, note);
});

test('the captured model repeats a requirement rather than stating the source fact', async () => {
  const capture = JSON.parse(
    await readFile(
      new URL('../fixtures/generation/body-echoed-missing-note.txt', import.meta.url),
      'utf8',
    ),
  );
  const note = capture.block.markdown;
  assert(
    /예약[^\n]{0,80}(없|미정|미기재|기재되어있지|정해지지)/.test(note.replace(/[\s,*_]/g, '')),
  );
  assert.equal(hasReservationAbsenceStatement(note), false);
});

test('the runner retains all its original fact checks and rejects the captured echo', async () => {
  const source = await readFile(new URL('../native/real-generation.mjs', import.meta.url), 'utf8');
  const actual = source.match(/function assertFacts\(text\) \{[\s\S]*?\n\}/)?.[0];
  assert(actual);
  const check = new Function('assert', 'hasReservationAbsenceStatement', `return (${actual})`)(
    assert,
    hasReservationAbsenceStatement,
  );
  for (const note of validNotes) check(facts + note);
  for (const note of echoedInstructions)
    assert.throws(() => check(facts + note), /reservation number is absent/);
  for (const [from, to] of [
    ['347000', '190000'],
    ['7명', '8명'],
    ['부산 해운대', '제주'],
    ['2026년 12월 5일', '2025년 12월 5일'],
  ])
    assert.throws(() => check((facts + validNotes[0]).replace(from, to)));
});

test('empty and non-text values cannot satisfy the note assertion', () => {
  for (const value of [null, undefined, 0, {}, '', '예약 번호를 만들지 마세요.'])
    assert.equal(hasReservationAbsenceStatement(value), false);
});

test('native evidence freezes the exact bytes of every directly imported local helper', async (t) => {
  const moduleUrl = new URL('../native/real-generation.mjs', import.meta.url);
  const source = await readFile(moduleUrl, 'utf8');
  const begin = '\n  for (const path of [\n    fileURLToPath(import.meta.url),';
  assert.equal(source.split(begin).length, 2);
  const start = source.indexOf(begin) + 1;
  const end = source.indexOf('\n  if (!verifyOnly) {', start);
  assert(end > start);
  const code = source.slice(start, end).replaceAll('import.meta.url', 'moduleUrl');
  const frozen = await mkdtemp(join(tmpdir(), 'prism-generation-input-evidence-'));
  t.after(async () => {
    for (const name of await readdir(frozen)) {
      const path = join(frozen, name);
      assert.equal(dirname(path), frozen);
      await unlink(path);
    }
    await rmdir(frozen);
  });
  const originals = [];
  const inspect = async (path, pin) => {
    const bytes = await readFile(path);
    const result = {
      path,
      bytes: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
    };
    if (pin) {
      assert.equal(result.bytes, pin.bytes);
      assert.equal(result.sha256, pin.sha256);
    }
    return result;
  };
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const capture = new AsyncFunction(
    'moduleUrl',
    'fileURLToPath',
    'fixtures',
    'inspect',
    'originals',
    'join',
    'frozen',
    'basename',
    'copyFile',
    code,
  );
  await capture(
    moduleUrl,
    fileURLToPath,
    fileURLToPath(new URL('../fixtures/generation/', import.meta.url)),
    inspect,
    originals,
    join,
    frozen,
    basename,
    copyFile,
  );
  const imports = [...source.matchAll(/^import\s+[^;]*?\s+from\s+(['"])(\.\/[^'"]+)\1;/gm)].map(
    (match) => match[2],
  );
  assert(imports.length >= 5, 'Expected the runner local helper imports');
  for (const specifier of imports) {
    const path = fileURLToPath(new URL(specifier, moduleUrl));
    assert(
      originals.some((record) => record.path === path),
      `Missing original hash: ${specifier}`,
    );
    const snapshot = join(frozen, basename(path));
    assert(
      originals.some((record) => record.path === snapshot),
      `Missing frozen hash: ${specifier}`,
    );
    assert.deepEqual(await readFile(snapshot), await readFile(path));
  }
});
