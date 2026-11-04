import { useEffect, useId, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { native } from './bridge';
import { setSession, useClient } from './store';

export interface SourceFingerprint {
  sha256: string;
  sizeBytes: number;
  modifiedUnixMs: number | null;
}
export interface ContentChunk {
  id: string;
  filePath: string;
  fingerprint: SourceFingerprint;
  sectionIndex: number;
  chunkIndex: number;
  text: string;
  page: number | null;
  paragraph: number | null;
  heading: string | null;
  charStart: number;
  charEnd: number;
}
export interface ContentHit {
  chunk: ContentChunk;
  score: number;
  snippet: string;
  keywordRank?: number | null;
  semanticRank?: number | null;
}
export interface ContentStatus {
  phase: string;
  discovered: number;
  indexed: number;
  skipped: number;
  embedded: number;
  revision: number;
  currentPath: string | null;
  issues: { path: string; code: string; message: string }[];
  embeddingError: string | null;
}
export interface SearchDiagnostics {
  candidateCount: number;
  candidateLimit: number;
  hardLimitReached: boolean;
}
interface TextResponse extends SearchDiagnostics {
  hits: ContentHit[];
}
interface SemanticResponse {
  hits: { chunk: ContentChunk; cosineDistance: number }[];
  candidateCount: number;
  candidateLimit: number;
  hardLimitReached: boolean;
  filteredOut: number;
  exhausted: boolean;
  totalVectors: number;
}
interface HybridResponse extends TextResponse {
  keywordCandidates: number;
  semanticCandidates: number;
  keywordSkipped: boolean;
}
export interface ContentFailure {
  code: string;
  message: string;
}
function failure(cause: unknown): ContentFailure {
  return cause && typeof cause === 'object' && 'code' in cause && 'message' in cause
    ? { code: String(cause.code), message: String(cause.message) }
    : { code: 'worker', message: String(cause) };
}
export const content = {
  status: () => invoke<ContentStatus>('content_status'),
  refresh: () => invoke<void>('content_refresh'),
  pause: () => invoke<void>('content_cancel_index'),
  cancel: (requestId: string) => invoke<void>('content_cancel_search', { requestId }),
  open: (path: string, reveal = false) => invoke<void>('content_file_action', { path, reveal }),
  chunks: (ids: string[]) => invoke<ContentChunk[]>('content_chunks', { ids }),
};
const emptyStatus: ContentStatus = {
  phase: 'ready',
  discovered: 0,
  indexed: 0,
  skipped: 0,
  embedded: 0,
  revision: 0,
  currentPath: null,
  issues: [],
  embeddingError: null,
};
export function useContentSearch(active: boolean) {
  const { settings, session } = useClient();
  const listId = useId();
  const scopes = settings.scopes.filter(
    (scope) => scope.contentEnabled && (!session.scopeId || scope.id === session.scopeId),
  );
  const scopesKey = scopes.map((scope) => `${scope.id}:${scope.path}`).join('|');
  const key = JSON.stringify([
    session.mode,
    session.query,
    session.extension,
    session.scopeId,
    scopesKey,
    settings.hybridSearch,
  ]);
  const [status, setStatus] = useState(emptyStatus);
  const [statusError, setStatusError] = useState<ContentFailure | null>(null);
  const [response, setResponse] = useState<{
    key: string;
    hits: ContentHit[];
    diagnostics: SearchDiagnostics;
    hybrid: boolean;
    keywordSkipped: boolean;
  } | null>(null);
  const [error, setError] = useState<{ key: string; error: ContentFailure } | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [selectedId, selectId] = useState<string | null>(null);
  useEffect(() => {
    if (!active || !native) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout>;
    const update = (next: ContentStatus) => {
      if (!disposed) {
        setStatus((previous) => (next.revision >= previous.revision ? next : previous));
        setStatusError(null);
      }
    };
    void listen<ContentStatus>('prism:content', (event) => update(event.payload))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch((cause) => {
        if (!disposed) setStatusError(failure(cause));
      });
    const poll = async () => {
      try {
        update(await content.status());
      } catch (cause) {
        if (!disposed) setStatusError(failure(cause));
      }
      if (!disposed) timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => {
      disposed = true;
      stop?.();
      clearTimeout(timer);
    };
  }, [active]);
  useEffect(() => {
    if (!active || !native || !session.query.trim() || !scopesKey) return;
    let disposed = false;
    let started = false;
    const requestId = crypto.randomUUID();
    setPending(key);
    const timer = setTimeout(
      () => {
        started = true;
        const args = {
          query: session.query,
          extension: session.extension,
          scopeId: session.scopeId,
          requestId,
        };
        const request =
          session.mode === 'semantic'
            ? settings.hybridSearch
              ? invoke<HybridResponse>('search_hybrid', args).then((data) => ({
                  hits: data.hits,
                  diagnostics: data,
                  hybrid: true,
                  keywordSkipped: data.keywordSkipped,
                }))
              : invoke<SemanticResponse>('search_semantic', args).then((data) => ({
                  hits: data.hits.map((hit) => ({
                    chunk: hit.chunk,
                    score: 1 - hit.cosineDistance,
                    snippet: Array.from(hit.chunk.text).slice(0, 320).join(''),
                  })),
                  diagnostics: {
                    candidateCount: data.candidateCount,
                    candidateLimit: data.candidateLimit,
                    hardLimitReached: data.hardLimitReached,
                  },
                  hybrid: false,
                  keywordSkipped: false,
                }))
            : invoke<ContentHit[] | TextResponse>('search_content', args).then((data) =>
                Array.isArray(data)
                  ? {
                      hits: data,
                      diagnostics: {
                        candidateCount: data.length,
                        candidateLimit: 100,
                        hardLimitReached: data.length >= 100,
                      },
                      hybrid: false,
                      keywordSkipped: false,
                    }
                  : { hits: data.hits, diagnostics: data, hybrid: false, keywordSkipped: false },
              );
        void request
          .then((data) => {
            if (!disposed) {
              setResponse({ key, ...data });
              setError(null);
            }
          })
          .catch((cause) => {
            const error = failure(cause);
            if (!disposed && error.code !== 'cancelled') setError({ key, error });
          })
          .finally(() => {
            if (!disposed) setPending((previous) => (previous === key ? null : previous));
          });
      },
      session.mode === 'semantic' ? 450 : 200,
    );
    return () => {
      disposed = true;
      clearTimeout(timer);
      if (started) void content.cancel(requestId).catch(() => {});
    };
  }, [
    active,
    key,
    scopesKey,
    session.mode,
    session.query,
    session.extension,
    session.scopeId,
    status.revision,
    settings.hybridSearch,
  ]);
  const data = response?.key === key ? response : null;
  const currentError = error?.key === key ? error.error : null;
  const selected =
    data?.hits.find((hit) => hit.chunk.id === selectedId) ??
    data?.hits.find((hit) => hit.chunk.filePath === session.selectedPath) ??
    data?.hits[0];
  const select = (id: string) => {
    selectId(id);
    const hit = data?.hits.find((hit) => hit.chunk.id === id);
    if (hit) setSession({ selectedPath: hit.chunk.filePath });
  };
  const move = (delta: number) => {
    if (!data?.hits.length) return;
    const index = data.hits.findIndex((hit) => hit.chunk.id === selected?.chunk.id);
    select(data.hits[(index + delta + data.hits.length) % data.hits.length].chunk.id);
  };
  return {
    listId,
    status,
    statusError,
    data,
    error: currentError,
    loading: pending === key,
    selected,
    select,
    move,
    scopes,
    mode: session.mode,
    query: session.query,
    open: (path: string, reveal = false) => content.open(path, reveal),
  };
}
export type ContentController = ReturnType<typeof useContentSearch>;
