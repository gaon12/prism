import { describe, expect, it } from 'vitest';
import { drawnRows, parentFolder, RESULT_PAGE } from './result-window';

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

describe('parentFolder', () => {
  it('drops the name and its separator', () => {
    expect(parentFolder('C:\\Users\\me\\report.md', 'report.md')).toBe('C:\\Users\\me');
    expect(parentFolder('/home/me/report.md', 'report.md')).toBe('/home/me');
  });

  it('keeps a root separator', () => {
    expect(parentFolder('C:\\report.md', 'report.md')).toBe('C:\\');
    expect(parentFolder('/report.md', 'report.md')).toBe('/');
  });

  it('shows the whole path when the name does not end it', () => {
    expect(parentFolder('C:\\a\\b', 'other')).toBe('C:\\a\\b');
    expect(parentFolder('C:\\a\\b', '')).toBe('C:\\a\\b');
  });
});
