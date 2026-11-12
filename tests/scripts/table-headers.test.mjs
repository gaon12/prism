import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { hasTableHeaders } from '../native/support/table-headers.mjs';

test('the requested column names match independently of the table values', () => {
  const table = [
    ['항목', '값'],
    ['예산 총액', '347000원'],
    ['참가 인원', '7명'],
  ];
  assert.equal(hasTableHeaders(table, ['항목', '값']), true);
  assert.equal(hasTableHeaders(table, ['재료', '분량']), false);
  assert.equal(hasTableHeaders(table, ['값', '항목']), false);
});

test('a captured real-model header leak fails despite correct values and two columns', async () => {
  const body = JSON.parse(
    await readFile(
      new URL('../fixtures/generation/body-copied-headers.txt', import.meta.url),
      'utf8',
    ),
  );
  const markdown = body.blocks.find((block) => block.markdown.includes('| 재료 |')).markdown;
  const rows = markdown
    .split('\n')
    .filter((line) => line.startsWith('|') && !line.includes('---'))
    .map((line) => line.split('|').slice(1, -1));
  assert.equal(rows.length, 3);
  assert(rows.every((row) => row.length === 2));
  assert(markdown.includes('347000원') && markdown.includes('7명'));
  assert.equal(hasTableHeaders(rows, ['항목', '값']), false);
});

test('Unicode composition and layout whitespace preserve the same column names', () => {
  assert.equal(hasTableHeaders([[' 항\n목 '.normalize('NFD'), '\t값 ']], ['항목', '값']), true);
});

test('missing, extra and malformed header cells cannot satisfy the contract', () => {
  for (const table of [null, [], [null], [['항목']], [['항목', '값', '출처']], [['항목', null]]])
    assert.equal(hasTableHeaders(table, ['항목', '값']), false);
  assert.equal(hasTableHeaders([[]], []), false);
});

test('the native runner enforces headers while retaining its row-count and value checks', async () => {
  const source = await readFile(new URL('../native/real-generation.mjs', import.meta.url), 'utf8');
  // Execute the runner's actual pure table assertion; do not start its app,
  // model or asset lifecycle, or maintain a second copy of the assertion.
  const begin = '\n    assert(\n      result.bodyTables.some(';
  assert.equal(source.split(begin).length, 2);
  const start = source.indexOf(begin) + 1;
  const end = source.indexOf('\n    );', start) + '\n    );'.length;
  assert(end > start);
  const check = new Function('result', 'assert', 'hasTableHeaders', source.slice(start, end));
  const valid = [
    ['항목', '값'],
    ['예산', '347000원'],
    ['참가 인원', '7명'],
  ];
  check({ bodyTables: [valid] }, assert, hasTableHeaders);
  for (const table of [
    [['재료', '분량'], ...valid.slice(1)],
    valid.slice(0, 2),
    [valid[0], ['예산', '190000원'], valid[2]],
    [valid[0], valid[1], ['참가 인원', '8명']],
    [valid[0], [...valid[1], '출처'], valid[2]],
  ]) {
    assert.throws(
      () => check({ bodyTables: [table] }, assert, hasTableHeaders),
      /reviewed plan requires/,
    );
  }
});
