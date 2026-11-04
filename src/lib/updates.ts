import { useEffect, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { native } from './bridge';

export interface UpdateView {
  autoCheck: boolean;
  checking: boolean;
  installing: boolean;
  available: { version: string; notes: string } | null;
  dismissedVersion: string | null;
  canInstall: boolean;
  lastCheckMs: number | null;
  checkError: string | null;
  installError: string | null;
  currentVersion: string;
  downloadPage: string;
  installDisabledReason: string | null;
  stateError: boolean;
}
let state: { view: UpdateView | null; error: boolean } = { view: null, error: false };
const listeners = new Set<() => void>();
const update = (next: typeof state) => {
  state = next;
  for (const listener of listeners) listener();
};
let started = false;
let epoch = 0;
let stop: (() => void) | undefined;
async function refresh() {
  const before = epoch;
  try {
    const view = await invoke<UpdateView>('update_status');
    if (before === epoch) update({ view, error: false });
  } catch {
    if (before === epoch) update({ ...state, error: true });
  }
}
async function initialize() {
  if (!native || started) return;
  started = true;
  try {
    stop = await listen<UpdateView>('prism:updates', (event) => {
      epoch++;
      update({ view: event.payload, error: false });
    });
    await refresh();
  } catch {
    update({ ...state, error: true });
  }
}
if (import.meta.hot) import.meta.hot.dispose(() => stop?.());
export function useUpdates() {
  const snapshot = useSyncExternalStore(
    (callback) => {
      listeners.add(callback);
      return () => listeners.delete(callback);
    },
    () => state,
  );
  useEffect(() => {
    void initialize();
  }, []);
  return snapshot;
}
export const updates = {
  async check() {
    const result = await invoke<UpdateView>('update_check');
    await refresh();
    return result;
  },
  async dismiss() {
    await invoke('update_dismiss');
    await refresh();
  },
  async install() {
    try {
      await invoke('update_install');
    } finally {
      await refresh();
    }
  },
  openDownload: () => invoke<void>('update_open_download'),
};
