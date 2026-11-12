import { describe, expect, it } from 'vitest';
import { drawnRows, nearEnd, RESULT_PAGE } from './result-window';

describe('drawnRows', () => {
  it('draws one page of a long result list at first', () => {
    expect(drawnRows(200, RESULT_PAGE, -1)).toBe(RESULT_PAGE);
    expect(drawnRows(200, RESULT_PAGE, 0)).toBe(RESULT_PAGE);
  });

  it('never draws more rows than there are', () => {
    expect(drawnRows(7, RESULT_PAGE, 6)).toBe(7);
    expect(drawnRows(0, RESULT_PAGE, -1)).toBe(0);
  });

  it('keeps what scrolling opened', () => {
    expect(drawnRows(200, 120, 3)).toBe(120);
  });

  it('includes a selection beyond the drawn rows, with room for the next arrow press', () => {
    // ArrowUp from the first row wraps to the last one.
    expect(drawnRows(200, RESULT_PAGE, 199)).toBe(200);
    expect(drawnRows(200, RESULT_PAGE, 45)).toBe(56);
  });
});

describe('nearEnd', () => {
  it('asks for more before the very end of the list', () => {
    // 600 px viewport over 2,000 px of rows.
    expect(nearEnd(0, 600, 2000)).toBe(false);
    expect(nearEnd(800, 600, 2000)).toBe(true);
  });

  it('asks for more when the rows do not even fill the viewport', () => {
    expect(nearEnd(0, 900, 700)).toBe(true);
  });
});
