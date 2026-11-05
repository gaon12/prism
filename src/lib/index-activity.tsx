import { createContext, useContext, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { bridge, native } from './bridge';
import type { IndexStatus } from './types';
import type { ContentStatus, ContentFailure } from './content';

interface IndexActivity {
  files: IndexStatus;
  content: ContentStatus;
  fileError: boolean;
  contentError: ContentFailure | null;
}
const initial: IndexActivity = {
  files: { scanning: false, count: 0, revision: 0, skipped: 0, warnings: [], watching: false },
  content: {
    phase: 'ready',
    discovered: 0,
    indexed: 0,
    skipped: 0,
    embedded: 0,
    revision: 0,
    currentPath: null,
    issues: [],
    embeddingError: null,
  },
  fileError: false,
  contentError: null,
};
const Context = createContext<IndexActivity | null>(null);
export const useIndexActivity = () => useContext(Context);
export const contentIndexBusy = (status: ContentStatus) =>
  ['scanning', 'indexing', 'embedding'].includes(status.phase);

/** One poll/listener per main window, shared by its search and task surfaces. */
export function IndexActivityProvider({
  active,
  children,
}: {
  active: boolean;
  children: ReactNode;
}) {
  const [state, setState] = useState(initial);
  useEffect(() => {
    if (!active || !native) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let stop: (() => void) | undefined;
    const acceptContent = (next: ContentStatus) => {
      if (!disposed)
        setState((previous) =>
          next.revision >= previous.content.revision
            ? { ...previous, content: next, contentError: null }
            : previous,
        );
    };
    void listen<ContentStatus>('prism:content', (event) => acceptContent(event.payload))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => {
        /* Polling below remains authoritative if events are unavailable. */
      });
    const poll = async () => {
      const [files, content] = await Promise.allSettled([
        bridge.indexStatus(),
        invoke<ContentStatus>('content_status'),
      ]);
      if (disposed) return;
      setState((previous) => ({
        files: files.status === 'fulfilled' ? files.value : previous.files,
        fileError: files.status === 'rejected',
        content:
          content.status === 'fulfilled' && content.value.revision >= previous.content.revision
            ? content.value
            : previous.content,
        contentError:
          content.status === 'rejected'
            ? { code: 'worker', message: String(content.reason?.message ?? content.reason) }
            : null,
      }));
      const busy =
        (files.status === 'fulfilled' && files.value.scanning) ||
        (content.status === 'fulfilled' && contentIndexBusy(content.value));
      timer = setTimeout(() => void poll(), busy ? 400 : 1500);
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
      stop?.();
    };
  }, [active]);
  return <Context.Provider value={active && native ? state : null}>{children}</Context.Provider>;
}
