import { useRef } from 'react';
import type { ClientSettings, SearchSession } from './types';
import type { ChatSelection } from './rag';
import { selectionForSearch } from './search-selection';
import { contentIndexBusy, useIndexActivity } from './index-activity';
import type { useSearch } from './use-search';

/**
 * What the "Chat with results" button may send, kept steady while indexing runs.
 *
 * Why this exists: indexing finishes in parts. After each part the index revision goes up,
 * the search runs again and is briefly "loading", and `selectionForSearch` returns null for
 * that moment. The button used to flip disabled → enabled → disabled with every part.
 *
 * Rules:
 * 1. While the same search refreshes, remember its last selection so the action's context
 *    stays steady. A revision-based selection remains disabled until fresh results arrive.
 * 2. A selection that names a whole result set by index revision (all matching filenames, or
 *    all keyword matches) can only be saved once that index stops changing: the native side
 *    rejects a revision that moved on ("Search results changed"). While that index is busy or
 *    the matching search response is still being refreshed, the
 *    button stays disabled and `waitingForIndex` explains why, instead of flickering.
 *    A fixed list of documents (ranked natural-language results) does not depend on a revision,
 *    so it stays available during indexing.
 */
export function useChatSelection(
  search: ReturnType<typeof useSearch>,
  session: SearchSession,
  settings: ClientSettings,
): { selection: ChatSelection | null; waitingForIndex: boolean } {
  const activity = useIndexActivity();
  const current = selectionForSearch(search, session, settings);
  const key = JSON.stringify([
    session.query,
    session.mode,
    session.extension,
    session.scopeId,
    settings.revision,
  ]);
  const last = useRef<{ key: string; selection: ChatSelection } | null>(null);
  // Remembering the newest real selection during render is safe: it only ever stores a value
  // this same render computed, so repeated renders give the same result.
  if (current) last.current = { key, selection: current };
  const refreshing = search.loading || (search.bodyActive && search.body.loading);
  const shown =
    current ?? (refreshing && last.current?.key === key ? last.current.selection : null);
  if (!shown) return { selection: null, waitingForIndex: false };

  const filesBusy = activity ? activity.files.scanning : search.status.scanning;
  const contentBusy = activity ? contentIndexBusy(activity.content) : false;
  // A watcher can finish updating the index before its new search reply arrives.
  // Keep the row mounted, but never enable an older whole-result-set descriptor
  // merely because scanning has already stopped. Check the revision during render
  // as well, before the search effect has had a chance to set its loading flag.
  const waitingForIndex = Boolean(
    (shown.filenameSearch &&
      (filesBusy || refreshing || shown.filenameSearch.indexRevision !== search.status.revision)) ||
    (shown.keywordSearch &&
      (contentBusy ||
        refreshing ||
        (activity && shown.keywordSearch.contentRevision !== activity.content.revision))),
  );
  return { selection: waitingForIndex ? null : shown, waitingForIndex };
}
