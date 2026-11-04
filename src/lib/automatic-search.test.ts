import { describe, expect, it } from 'vitest';
import { automaticBodyEligible } from './automatic-search';
describe('automatic body fallback', () => {
  it('keeps empty queries, explicit filters and paths on filename search', () => {
    for (const value of [
      '',
      '  ',
      'ext:pdf 여행',
      'extension:hwpx edit-date:>2023 공문서',
      'PATH:회의',
      'name:budget',
      'C:\\자료',
      '\\\\server\\share',
      '/home/me',
      'folder/report.md',
      '"C:\\문서 모음"',
      '"ext:pdf" 출장',
      'ｅｘｔ：pdf 출장',
    ])
      expect(automaticBodyEligible(value), value).toBe(false);
  });
  it('does not depend on language-specific question heuristics', () => {
    for (const value of [
      '출장 경비',
      '여름휴가 일정 어디에 있어?',
      'travel budget',
      '出差费用',
      '出張の予算',
      '"meeting: notes"',
    ])
      expect(automaticBodyEligible(value), value).toBe(true);
  });
});
