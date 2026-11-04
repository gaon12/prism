import { useSyncExternalStore } from 'react';
import { bridge } from './bridge';
import { defaultSettings, emptySession, rememberQuery, scopeName, scopePathKey } from './types';
import type { ClientSettings, RuntimeInfo, SearchScope, SearchSession } from './types';

interface ClientState {
  ready: boolean;
  error: string | null;
  settings: ClientSettings;
  session: SearchSession;
  runtime: RuntimeInfo;
  toast: string | null;
  focusTick: number;
}
let state: ClientState = {
  ready: false,
  error: null,
  settings: structuredClone(defaultSettings),
  session: structuredClone(emptySession),
  runtime: { native: false, development: true, shortcutError: null, startupError: null },
  toast: null,
  focusTick: 0,
};
const listeners = new Set<() => void>();
function update(patch: Partial<ClientState>) {
  state = { ...state, ...patch };
  listeners.forEach((listener) => listener());
}
export function useClient() {
  return useSyncExternalStore(
    (callback) => {
      listeners.add(callback);
      return () => listeners.delete(callback);
    },
    () => state,
  );
}
let initializePromise: Promise<void> | null = null;
let sessionTimer: ReturnType<typeof setTimeout> | undefined;
let sessionWrite: Promise<void> = Promise.resolve();

export function showToast(message: string) {
  update({ toast: message });
}
export function clearToast() {
  update({ toast: null });
}
export function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function initialize() {
  if (initializePromise) return initializePromise;
  initializePromise = (async () => {
    let unsubscribe: (() => void) | undefined;
    try {
      unsubscribe = await bridge.subscribe(
        (settings) => update({ settings }),
        (session) => update({ session }),
        () => update({ focusTick: state.focusTick + 1 }),
      );
      const [settings, session, runtime] = await Promise.all([
        bridge.getSettings(),
        bridge.getSession(),
        bridge.runtime(),
      ]);
      update({
        settings,
        session: settings.rememberSearch ? session : structuredClone(emptySession),
        runtime,
        ready: true,
        error: null,
      });
    } catch (error) {
      unsubscribe?.();
      update({ error: errorMessage(error), ready: false });
      initializePromise = null;
    }
  })();
  return initializePromise;
}

export function setSession(patch: Partial<SearchSession>) {
  update({ session: { ...state.session, ...patch } });
  clearTimeout(sessionTimer);
  sessionTimer = setTimeout(() => {
    void flushSession().catch((error) => showToast(errorMessage(error)));
  }, 180);
}
export async function flushSession() {
  clearTimeout(sessionTimer);
  const snapshot = { ...state.session };
  sessionWrite = sessionWrite.catch(() => undefined).then(() => bridge.saveSession(snapshot));
  await sessionWrite;
}
export function submitSearch() {
  if (state.settings.rememberSearch) update({ session: rememberQuery(state.session) });
  return flushSession();
}
export async function saveSettings(settings: ClientSettings) {
  const saved = await bridge.saveSettings(settings);
  update({ settings: saved });
  if (!saved.rememberSearch) setSession({ recentQueries: [] });
  if (state.session.scopeId && !saved.scopes.some((scope) => scope.id === state.session.scopeId))
    setSession({ scopeId: null, selectedPath: null });
}
export function scopesFromPaths(
  paths: string[],
  existing: SearchScope[] = state.settings.scopes,
): SearchScope[] {
  const normalize = scopePathKey;
  const known = new Set(existing.map((scope) => normalize(scope.path)));
  const added: SearchScope[] = [];
  for (const path of paths) {
    if (known.has(normalize(path))) continue;
    known.add(normalize(path));
    added.push({
      id: crypto.randomUUID(),
      name: scopeName(path),
      path,
      filenameEnabled: true,
      contentEnabled: false,
      externalAllowed: false,
    });
  }
  return [...existing, ...added];
}
export async function addFolders() {
  const paths = await bridge.chooseFolders();
  if (!paths.length) return false;
  await saveSettings({ ...state.settings, scopes: scopesFromPaths(paths) });
  return true;
}
export async function switchWindow(mode: 'main' | 'quick') {
  if (mode === 'quick' && !state.settings.rememberSearch)
    setSession({ query: '', selectedPath: null });
  await flushSession();
  if (mode === 'quick') await bridge.showQuick();
  else await bridge.showMain();
}
