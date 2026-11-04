import { describe, expect, it } from 'vitest';
import { matchedSegments } from './text-match';

describe('source-preserving content highlights', () => {
  it('marks every term and merges overlapping matches', () => {
    expect(
      matchedSegments('제주 경비와 제주도', '제주 제주도 경비')
        .filter((part) => part.matched)
        .map((part) => part.text),
    ).toEqual(['제주', '경비', '제주도']);
  });
  it('maps compatibility and decomposed Korean to original graphemes', () => {
    const original = 'ＡＢＣ 제주 여행';
    const parts = matchedSegments(original, 'abc 제주');
    expect(parts.map((part) => part.text).join('')).toBe(original);
    expect(parts.filter((part) => part.matched).map((part) => part.text)).toEqual([
      'ＡＢＣ',
      '제주',
    ]);
  });
  it('matches the whitespace-insensitive backend projection', () => {
    expect(matchedSegments('한국 어 문서', '한국어')[0]).toEqual({
      text: '한국 어',
      matched: true,
    });
  });
  it('preserves literal markup, emoji and unmatched text', () => {
    const text = '<img src=x> 🍎 abc';
    const parts = matchedSegments(text, '🍎 abc');
    expect(parts.map((part) => part.text).join('')).toBe(text);
    expect(parts[0]).toEqual({ text: '<img src=x> ', matched: false });
    expect(matchedSegments(text, ' ')).toEqual([{ text, matched: false }]);
  });
});
