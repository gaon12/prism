import { describe, expect, it } from 'vitest';
import { nextSort, sortResults } from './result-sort';
import type { FileResult } from './types';

const file = (
  path: string,
  size: number,
  modifiedMs: number | null,
  isDir = false,
): FileResult => ({
  path,
  name: path.split('\\').pop()!,
  isDir,
  size,
  modifiedMs,
});
// The search's own order (best match first).
const ranked = [
  file('C:\\b\\report10.md', 10, 300),
  file('C:\\a\\Report2.md', 30, null),
  file('C:\\c\\notes', 0, 200, true),
  file('C:\\a\\report2.md', 20, 100),
];
const paths = (files: FileResult[]) => files.map((item) => item.path);

describe('nextSort', () => {
  it('goes ascending, descending, then back to best match', () => {
    const first = nextSort(null, 'name');
    expect(first).toEqual({ key: 'name', descending: false });
    const second = nextSort(first, 'name');
    expect(second).toEqual({ key: 'name', descending: true });
    expect(nextSort(second, 'name')).toBeNull();
    expect(nextSort(second, 'size')).toEqual({ key: 'size', descending: false });
  });
});

describe('sortResults', () => {
  it('keeps the search order without a sort and never changes the input', () => {
    const copy = [...ranked];
    expect(sortResults(ranked, null)).toBe(ranked);
    sortResults(ranked, { key: 'name', descending: false });
    expect(ranked).toEqual(copy);
  });

  it('sorts names naturally and keeps the search order for equal names', () => {
    expect(paths(sortResults(ranked, { key: 'name', descending: false }))).toEqual([
      'C:\\c\\notes',
      'C:\\a\\Report2.md',
      'C:\\a\\report2.md',
      'C:\\b\\report10.md',
    ]);
  });

  it('puts folders first by size and unknown dates last in both directions', () => {
    expect(paths(sortResults(ranked, { key: 'size', descending: false }))).toEqual([
      'C:\\c\\notes',
      'C:\\b\\report10.md',
      'C:\\a\\report2.md',
      'C:\\a\\Report2.md',
    ]);
    for (const descending of [false, true])
      expect(paths(sortResults(ranked, { key: 'modified', descending })).at(-1)).toBe(
        'C:\\a\\Report2.md',
      );
    expect(paths(sortResults(ranked, { key: 'modified', descending: true }))[0]).toBe(
      'C:\\b\\report10.md',
    );
  });

  it('sorts by the folder that holds each result', () => {
    expect(paths(sortResults(ranked, { key: 'folder', descending: true }))).toEqual([
      'C:\\c\\notes',
      'C:\\b\\report10.md',
      'C:\\a\\Report2.md',
      'C:\\a\\report2.md',
    ]);
  });
});
