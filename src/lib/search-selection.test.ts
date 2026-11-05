import { describe, expect, it } from 'vitest';
import { searchSelection, selectionForSearch } from './search-selection';
import { defaultSettings, emptySession } from './types';

describe('search result handoff', () => {
  it('uses an exact keyword descriptor while preserving bounded semantic previews', () => {
    const preview: Parameters<typeof selectionForSearch>[0] = {
      request: { kind: 'files', input: 'entryneedle' },
      bodyActive: true,
      loading: false,
      error: null,
      data: null,
      body: {
        loading: false,
        error: null,
        data: {
          key: 'k',
          contentRevision: 7,
          hits: [
            {
              chunk: {
                id: 'a',
                filePath: '/a.md',
                fingerprint: { sha256: 'x', sizeBytes: 1, modifiedUnixMs: null },
                sectionIndex: 0,
                chunkIndex: 0,
                text: 'not sent',
                page: null,
                paragraph: null,
                heading: null,
                charStart: 0,
                charEnd: 8,
              },
              score: 1,
              snippet: 'not sent',
            },
          ],
          diagnostics: { candidateCount: 1, candidateLimit: 100, hardLimitReached: false },
          hybrid: false,
          keywordSkipped: false,
          automatic: null,
        },
      },
    };
    const session = {
      ...emptySession,
      mode: 'content' as const,
      query: 'entryneedle',
      extension: 'md',
      scopeId: 'docs',
    };
    expect(
      selectionForSearch(preview, session, { ...defaultSettings, revision: 3 })?.keywordSearch,
    ).toEqual({
      query: 'entryneedle',
      extension: 'md',
      scopeId: 'docs',
      contentRevision: 7,
      settingsRevision: 3,
    });
    const semantic = selectionForSearch(
      preview,
      { ...session, mode: 'semantic' },
      defaultSettings,
    )!;
    expect(semantic.keywordSearch).toBeUndefined();
    expect(semantic.documents).toEqual(['/a.md']);
    expect(JSON.stringify(semantic)).not.toContain('not sent');
    preview.body.data!.automatic = {
      ranking: 'keyword',
      semanticState: 'notRunning',
      semanticFailure: null,
    };
    expect(
      selectionForSearch(preview, { ...session, mode: 'auto' }, defaultSettings)?.keywordSearch
        ?.contentRevision,
    ).toBe(7);
    preview.body.data!.automatic = {
      ranking: 'hybrid',
      semanticState: 'used',
      semanticFailure: null,
    };
    expect(
      selectionForSearch(preview, { ...session, mode: 'auto' }, defaultSettings)?.keywordSearch,
    ).toBeUndefined();
    preview.body.loading = true;
    expect(selectionForSearch(preview, session, defaultSettings)).toBeNull();
  });
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
