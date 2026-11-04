import { describe, expect, it } from 'vitest';
import { scalarSelection } from './documents';

describe('document selection offsets', () => {
  it('converts DOM UTF-16 selections to Rust scalar ranges without changing Korean text', () => {
    const text = 'A🙂한글Z';
    expect(scalarSelection(text, 1, 4)).toEqual({ start: 1, end: 3 });
    expect(scalarSelection(text, 3, 5)).toEqual({ start: 2, end: 4 });
    expect(scalarSelection(text, 0, text.length)).toEqual({ start: 0, end: 5 });
  });

  it('rejects a cursor inside an emoji, empty ranges and invalid DOM offsets', () => {
    const text = 'A🙂한글Z';
    for (const [start, end] of [
      [1, 2],
      [2, 4],
      [3, 3],
      [4, 3],
      [-1, 3],
      [0, 7],
      [0.5, 3],
      [0, NaN],
    ]) {
      expect(scalarSelection(text, start, end)).toBeNull();
    }
  });
});
