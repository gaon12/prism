import { useEffect, useId, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { native } from './bridge';
import { errorMessage } from './store';

export type ClipKind = 'text' | 'link' | 'image';
export interface ClipItem {
  id: string;
  kind: ClipKind;
  preview: string;
  byteSize: number;
  width?: number;
  height?: number;
  createdMs: number;
  lastCopiedMs: number;
  copyCount: number;
  pinned: boolean;
}
export interface ClipStatus {
  enabled: boolean;
  paused: boolean;
  count: number;
  unpinnedCount: number;
  bytes: number;
  revision: number;
  lastError: string | null;
}
interface ClipList {
  items: ClipItem[];
  total: number;
  status: ClipStatus;
}
export const clipboard = {
  list: (query: string, kind: ClipKind | null) =>
    invoke<ClipList>('clipboard_list', { query, kind, limit: 100 }),
  copy: (id: string) => invoke<void>('clipboard_copy', { id }),
  pin: (id: string, pinned: boolean) => invoke<ClipItem>('clipboard_pin', { id, pinned }),
  delete: (id: string) => invoke<void>('clipboard_delete', { id }),
  clear: (includePinned = false) => invoke<void>('clipboard_clear', { includePinned }),
  pause: (paused: boolean) => invoke<ClipStatus>('clipboard_set_paused', { paused }),
  thumbnail: (id: string) => invoke<string>('clipboard_thumbnail', { id }),
  text: (id: string) => invoke<string>('clipboard_text', { id }),
};

export function useClipboard(active: boolean, query: string) {
  const listId = useId();
  const [kind, setKind] = useState<ClipKind | null>(null);
  const [tick, setTick] = useState(0);
  const [response, setResponse] = useState<{ key: string; data: ClipList } | null>(null);
  const [failure, setFailure] = useState<{ key: string; message: string } | null>(null);
  const [selectedId, select] = useState<string | null>(null);
  const key = JSON.stringify([query, kind]);
  useEffect(() => {
    if (!active || !native) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<ClipStatus>('prism:clipboard', () => setTick((value) => value + 1))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch((cause) => {
        if (!disposed) setFailure({ key, message: errorMessage(cause) });
      });
    // Subscribe before the initial request. Also refresh after a hidden window becomes visible.
    const focus = () => setTick((value) => value + 1);
    window.addEventListener('focus', focus);
    return () => {
      disposed = true;
      window.removeEventListener('focus', focus);
      stop?.();
    };
  }, [active]);
  useEffect(() => {
    if (!active || !native) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      void clipboard
        .list(query, kind)
        .then((data) => {
          if (!cancelled) {
            setResponse({ key, data });
            setFailure(null);
          }
        })
        .catch((cause) => {
          if (!cancelled) setFailure({ key, message: errorMessage(cause) });
        });
    }, 80);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [active, key, query, kind, tick]);
  const data = response?.key === key ? response.data : null;
  const error = failure?.key === key ? failure.message : null;
  const selected = data?.items.find((item) => item.id === selectedId) ?? data?.items[0];
  const move = (delta: number) => {
    if (!data?.items.length) return;
    const index = data.items.findIndex((item) => item.id === selected?.id);
    select(data.items[(index + delta + data.items.length) % data.items.length].id);
  };
  return {
    listId,
    data,
    error,
    selected,
    select,
    move,
    kind,
    setKind,
    refresh: () => setTick((value) => value + 1),
  };
}
export type ClipboardController = ReturnType<typeof useClipboard>;
