import { describe, expect, it } from 'vitest';
import { searchSelection } from './search-selection';

describe('search result handoff', () => {
  it('passes every returned file once without excerpts or automatic questions', () => {
    const value = searchSelection('query', ['/a.md', '/b.md', '/a.md'], true);
    expect(value).toEqual({
      query: 'query',
      documents: ['/a.md', '/b.md'],
      selectedDocuments: ['/a.md', '/b.md'],
      partial: true,
    });
    value!.selectedDocuments.pop();
    expect(value!.documents).toHaveLength(2);
    expect(searchSelection('', [])).toBeNull();
  });
});
