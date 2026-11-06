import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { open, save } from '@tauri-apps/plugin-dialog';
import { defaultSettings, emptySession, normalizeSettings, normalizeSession } from './types';
import type {
  ClientSettings,
  RecommendedFolder,
  RuntimeInfo,
  SearchSession,
  IndexStatus,
  SearchResponse,
} from './types';
import { webSearchUrl } from './utilities';
import type { WebProvider } from './utilities';

export const native = isTauri();
const settingsKey = 'prism.settings.v1';
const sessionKey = 'prism.session.v1';
function read<T>(key: string, fallback: T): T {
  try {
    const value = localStorage.getItem(key);
    return value ? (JSON.parse(value) as T) : structuredClone(fallback);
  } catch {
    return structuredClone(fallback);
  }
}

export const bridge = {
  async openSettings(tab: 'general' | 'scopes' | 'shortcuts' | 'ai') {
    if (native) await invoke('open_settings', { tab });
    else {
      window.location.hash = '';
      window.dispatchEvent(new CustomEvent('prism:open-settings', { detail: tab }));
    }
  },
  async subscribeSettings(handler: (tab: 'general' | 'scopes' | 'shortcuts' | 'ai') => void) {
    if (native)
      return getCurrentWebviewWindow().listen<'general' | 'scopes' | 'shortcuts' | 'ai'>(
        'prism:open-settings',
        (event) => handler(event.payload),
      );
    const listener = (event: Event) => handler((event as CustomEvent).detail);
    window.addEventListener('prism:open-settings', listener);
    return () => window.removeEventListener('prism:open-settings', listener);
  },
  async indexStatus(): Promise<IndexStatus> {
    return native
      ? invoke('index_status')
      : {
          scanning: false,
          initialScanComplete: false,
          count: 0,
          revision: 0,
          skipped: 0,
          warnings: [],
          watching: false,
        };
  },
  async searchFiles(
    query: string,
    extension: string,
    scopeId: string | null,
  ): Promise<SearchResponse> {
    if (!native) throw new Error('NATIVE_ONLY');
    return invoke('search_files', { query, extension, scopeId });
  },
  async refreshIndex() {
    if (!native) throw new Error('NATIVE_ONLY');
    await invoke('refresh_index');
  },
  async fileAction(path: string, reveal = false) {
    if (!native) throw new Error('NATIVE_ONLY');
    await invoke('file_action', { path, reveal });
  },
  async copyText(text: string) {
    if (native) await invoke('copy_text', { text });
    else await navigator.clipboard.writeText(text);
  },
  async webSearch(provider: WebProvider, query: string) {
    if (!navigator.onLine) throw new Error('OFFLINE');
    if (native) await invoke('open_web_search', { provider, query });
    else window.open(webSearchUrl(provider, query), '_blank', 'noopener,noreferrer');
  },
  async getSettings(): Promise<ClientSettings> {
    return native ? invoke('get_settings') : normalizeSettings(read(settingsKey, defaultSettings));
  },
  async saveSettings(settings: ClientSettings): Promise<ClientSettings> {
    if (native) return invoke('save_settings', { settings, expectedRevision: settings.revision });
    const previous = normalizeSettings(read(settingsKey, defaultSettings));
    if (previous.revision !== settings.revision)
      throw new Error('Settings changed in another window. Reopen settings and try again.');
    const next = { ...normalizeSettings(settings), revision: previous.revision + 1 };
    localStorage.setItem(settingsKey, JSON.stringify(next));
    if (!next.rememberSearch) localStorage.removeItem(sessionKey);
    return next;
  },
  async getSession(): Promise<SearchSession> {
    return native ? invoke('get_search_session') : normalizeSession(read(sessionKey, emptySession));
  },
  async saveSession(session: SearchSession) {
    if (native) await invoke('set_search_session', { session });
    else if (normalizeSettings(read(settingsKey, defaultSettings)).rememberSearch)
      localStorage.setItem(sessionKey, JSON.stringify(session));
  },
  async runtime(): Promise<RuntimeInfo> {
    return native
      ? invoke('runtime_info')
      : {
          native: false,
          development: true,
          installKind: null,
          shortcutError: null,
          startupError: null,
        };
  },
  async chooseFolders(): Promise<string[]> {
    if (!native) throw new Error('NATIVE_ONLY');
    const paths = await open({ directory: true, multiple: true });
    return paths ? (Array.isArray(paths) ? paths : [paths]) : [];
  },
  async recommendedFolders(): Promise<RecommendedFolder[]> {
    return native ? invoke('recommended_folders') : [];
  },
  async showQuick() {
    if (native) await invoke('show_quick');
    else window.location.hash = 'quick';
  },
  async resizeQuick(expanded: boolean) {
    if (native) await invoke('resize_quick', { expanded });
  },
  async showMain() {
    if (native) await invoke('show_main');
    else window.location.hash = '';
  },
  async hideQuick() {
    if (native) await invoke('hide_quick');
    else window.location.hash = '';
  },
  async windowAction(action: 'minimize' | 'maximize' | 'close') {
    if (native) await invoke('window_action', { action });
  },
  async quit() {
    if (native) await invoke('quit_app');
  },
  async exportMarkdown(title: string, markdown: string) {
    const safeTitle = title.replace(/[<>:"/\\|?*\x00-\x1f]/g, '-').trim() || 'untitled';
    const contents = markdown || `# ${title}\n`;
    if (native) {
      const path = await save({
        defaultPath: `${safeTitle}.md`,
        filters: [{ name: 'Markdown', extensions: ['md'] }],
      });
      if (!path) return false;
      await invoke('write_markdown', { path, contents });
    } else {
      const url = URL.createObjectURL(
        new Blob([contents], { type: 'text/markdown;charset=utf-8' }),
      );
      const link = document.createElement('a');
      link.href = url;
      link.download = `${safeTitle}.md`;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }
    return true;
  },
  async choosePdfPath(title: string): Promise<string | null> {
    if (!native) return null;
    let safeTitle =
      title
        .replace(/[<>:"/\\|?*\x00-\x1f]/g, '-')
        .trim()
        .replace(/[ .]+$/, '') || 'untitled';
    if (/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(safeTitle))
      safeTitle = `Prism-${safeTitle}`;
    const encoder = new TextEncoder();
    let shortened = '';
    for (const character of safeTitle) {
      if (encoder.encode(shortened + character).length > 180) break;
      shortened += character;
    }
    safeTitle = shortened || 'untitled';
    return save({
      defaultPath: `${safeTitle}.pdf`,
      filters: [{ name: 'PDF', extensions: ['pdf'] }],
    });
  },
  async subscribe(
    onSettings: (settings: ClientSettings) => void,
    onSession: (session: SearchSession) => void,
    onFocus: () => void,
  ) {
    if (native) {
      const unlisteners = await Promise.all([
        listen<ClientSettings>('prism:settings', (event) => onSettings(event.payload)),
        listen<SearchSession>('prism:session', (event) => onSession(event.payload)),
        getCurrentWebviewWindow().listen('prism:focus-search', onFocus),
      ]);
      return () => unlisteners.forEach((unlisten) => unlisten());
    }
    const handler = (event: StorageEvent) => {
      try {
        if (event.key === settingsKey && event.newValue)
          onSettings(normalizeSettings(JSON.parse(event.newValue)));
        if (event.key === sessionKey && event.newValue)
          onSession(normalizeSession(JSON.parse(event.newValue)));
      } catch {
        /* An invalid external storage change must not crash the current window. */
      }
    };
    window.addEventListener('storage', handler);
    return () => window.removeEventListener('storage', handler);
  },
};
