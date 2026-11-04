export type Language = 'ko' | 'en' | 'zh-CN' | 'zh-TW' | 'ja';
export type Theme = 'system' | 'light' | 'dark';
export type Tab = 'search' | 'chat' | 'documents';
export interface SearchScope {
  id: string;
  name: string;
  path: string;
  filenameEnabled: boolean;
  contentEnabled: boolean;
}
export interface ClientSettings {
  schemaVersion: number;
  revision: number;
  theme: Theme;
  language: Language;
  fontSize: number;
  shortcut: string;
  rememberSearch: boolean;
  closeToTray: boolean;
  startOnLogin: boolean;
  scopes: SearchScope[];
  calculator: { precision: number; angle: 'rad' | 'deg' };
}
export interface SearchSession {
  query: string;
  scopeId: string | null;
  extension: string;
  selectedPath: string | null;
  recentQueries: string[];
}
export interface RecommendedFolder {
  name: string;
  path: string;
}
export interface RuntimeInfo {
  native: boolean;
  development: boolean;
  shortcutError: string | null;
  startupError: string | null;
}
export interface FileResult {
  path: string;
  name: string;
  isDir: boolean;
  size: number;
  modifiedMs: number | null;
}
export interface IndexStatus {
  scanning: boolean;
  count: number;
  revision: number;
  skipped: number;
  warnings: string[];
  watching: boolean;
}
export interface SearchResponse {
  results: FileResult[];
  total: number;
  status: IndexStatus;
}
export const defaultSettings: ClientSettings = {
  schemaVersion: 1,
  revision: 0,
  theme: 'system',
  language: 'ko',
  fontSize: 14,
  shortcut: 'Alt+Space',
  rememberSearch: true,
  closeToTray: true,
  startOnLogin: true,
  scopes: [],
  calculator: { precision: 12, angle: 'rad' },
};
export const emptySession: SearchSession = {
  query: '',
  scopeId: null,
  extension: '',
  selectedPath: null,
  recentQueries: [],
};

export function normalizeSettings(input: unknown): ClientSettings {
  if (!input || typeof input !== 'object') return structuredClone(defaultSettings);
  const value = input as Partial<ClientSettings>;
  const language = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'].includes(value.language ?? '')
    ? value.language!
    : 'ko';
  const theme = ['light', 'dark', 'system'].includes(value.theme ?? '') ? value.theme! : 'system';
  return {
    ...defaultSettings,
    ...value,
    schemaVersion: 1,
    language,
    theme,
    revision: Number.isSafeInteger(value.revision) && value.revision! >= 0 ? value.revision! : 0,
    fontSize: Math.min(18, Math.max(12, Number(value.fontSize) || 14)),
    shortcut:
      typeof value.shortcut === 'string' && value.shortcut.trim()
        ? value.shortcut.trim()
        : defaultSettings.shortcut,
    rememberSearch: typeof value.rememberSearch === 'boolean' ? value.rememberSearch : true,
    closeToTray: typeof value.closeToTray === 'boolean' ? value.closeToTray : true,
    startOnLogin: typeof value.startOnLogin === 'boolean' ? value.startOnLogin : true,
    calculator: {
      precision: Math.min(15, Math.max(6, Math.round(Number(value.calculator?.precision) || 12))),
      angle: value.calculator?.angle === 'deg' ? 'deg' : 'rad',
    },
    scopes: Array.isArray(value.scopes)
      ? value.scopes
          .filter(
            (scope) =>
              typeof scope?.path === 'string' && scope.path && typeof scope?.id === 'string',
          )
          .map((scope) => ({
            ...scope,
            name: typeof scope.name === 'string' ? scope.name : scopeName(scope.path),
            filenameEnabled: scope.filenameEnabled !== false,
            contentEnabled: scope.contentEnabled === true,
          }))
      : [],
  };
}

export function rememberQuery(session: SearchSession): SearchSession {
  const query = session.query.trim();
  if (!query) return session;
  return {
    ...session,
    recentQueries: [query, ...session.recentQueries.filter((item) => item !== query)].slice(0, 20),
  };
}

export function scopeName(path: string) {
  return (
    path
      .replace(/[\\/]+$/, '')
      .split(/[\\/]/)
      .pop() || path
  );
}
export function isComposing(event: { isComposing?: boolean; keyCode?: number }) {
  return event.isComposing === true || event.keyCode === 229;
}

export function scopePathKey(path: string) {
  const key = path.replaceAll('\\', '/').replace(/\/+$/, '');
  return /^[a-z]:\//i.test(key) || key.startsWith('//') ? key.toLowerCase() : key;
}

export function normalizeSession(input: unknown): SearchSession {
  if (!input || typeof input !== 'object') return structuredClone(emptySession);
  const value = input as Partial<SearchSession>;
  return {
    query: typeof value.query === 'string' ? value.query : '',
    scopeId: typeof value.scopeId === 'string' ? value.scopeId : null,
    extension: typeof value.extension === 'string' ? value.extension : '',
    selectedPath: typeof value.selectedPath === 'string' ? value.selectedPath : null,
    recentQueries: Array.isArray(value.recentQueries)
      ? [
          ...new Set(value.recentQueries.filter((item) => typeof item === 'string' && item.trim())),
        ].slice(0, 20)
      : [],
  };
}
