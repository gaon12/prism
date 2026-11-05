import { describe, expect, it } from 'vitest';
import { preferSelectionPage, type SelectionPage } from './rag';
describe('selection state during transcript refresh', () => {
  it('retains a later file page and prevents an older response overwriting saved choices', () => {
    const current: SelectionPage = {
      query: 'q',
      partial: false,
      revision: 3,
      total: 1006,
      selectedCount: 1005,
      offset: 1000,
      hasMore: false,
      files: [],
    };
    expect(preferSelectionPage(current, { ...current, offset: 0 })).toBe(current);
    expect(preferSelectionPage(current, { ...current, revision: 2, selectedCount: 1006 })).toBe(
      current,
    );
    const newer = { ...current, revision: 4, selectedCount: 0, offset: 0 };
    expect(preferSelectionPage(current, newer)).toBe(newer);
    expect(preferSelectionPage(current, null)).toBeNull();
  });
});
