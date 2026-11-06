import type { ChatSelection } from './rag';
import type { ClientSettings, SearchSession } from './types';
import type { useSearch } from './use-search';

type SearchPreview = Pick<
  ReturnType<typeof useSearch>,
  'request' | 'bodyActive' | 'loading' | 'error' | 'data'
> & {
  body: Pick<ReturnType<typeof useSearch>['body'], 'data' | 'loading' | 'error'>;
};
/** Shared by the main and quick windows so they freeze the same query/filter. */
export function selectionForSearch(
  search: SearchPreview,
  session: SearchSession,
  settings: ClientSettings,
): ChatSelection | null {
  if (search.request.kind !== 'files') return null;
  if (search.bodyActive) {
    const data = search.body.data;
    if (search.body.loading || search.body.error || !data) return null;
    const selection = searchSelection(
      session.query,
      data.hits.map((hit) => hit.chunk.filePath),
      data.diagnostics.hardLimitReached || data.hits.length >= 100,
    );
    if (!selection) return null;
    // Keywords enumerate all matches beyond the display limit. Ranked search
    // hands off every distinct file in the shown top-N hits, as an explicit
    // fixed selection; it must never silently widen to other folder documents.
    if (session.mode === 'content' || data.automatic?.ranking === 'keyword') {
      selection.keywordSearch = {
        query: session.query,
        extension: session.extension,
        scopeId: session.scopeId,
        contentRevision: data.contentRevision,
        settingsRevision: settings.revision,
      };
    }
    return selection;
  }
  if (search.loading || search.error || !search.data || search.data.total === 0) return null;
  return {
    ...(searchSelection(
      session.query,
      search.data.results.filter((file) => !file.isDir).map((file) => file.path),
    ) ?? { query: session.query, documents: [], selectedDocuments: [], partial: false }),
    filenameSearch: {
      query: search.request.input,
      extension: session.extension,
      scopeId: session.scopeId,
      indexRevision: search.data.status.revision,
      settingsRevision: settings.revision,
    },
  };
}

/** Transfer identities, never excerpts or an implicit permission to read them. */
export function searchSelection(
  query: string,
  paths: string[],
  partial = false,
): ChatSelection | null {
  const documents = [...new Set(paths)];
  return documents.length ? { query, documents, selectedDocuments: [...documents], partial } : null;
}
