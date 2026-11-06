import { describe, expect, it } from 'vitest';
import { validResearchQuery } from './ExternalResearch';

describe('explicit document research query', () => {
  it('accepts a separate nonempty single-line search sentence within the UTF-8 budget', () => {
    for (const query of [
      '출장 관련 최신 자료',
      '  explicit query  ',
      'a'.repeat(4096),
      '가'.repeat(1365),
    ])
      expect(validResearchQuery(query)).toBe(true);
  });
  it('rejects empty, control-bearing and byte-overflowing queries before generation', () => {
    for (const query of ['', '  ', 'a\nb', 'a\u0085b', 'a\0b', 'a'.repeat(4097), '가'.repeat(1366)])
      expect(validResearchQuery(query)).toBe(false);
  });
});
