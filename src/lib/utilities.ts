export type UtilityKind = 'files' | 'calculator' | 'units' | 'web' | 'clipboard' | 'dictionary';
export type WebProvider = 'google' | 'bing' | 'ddg' | 'namu' | 'wiki';
export interface UtilityRequest {
  kind: UtilityKind;
  input: string;
  provider?: WebProvider;
  dictionaryLanguage?: 'ko' | 'en' | 'zh' | 'ja';
}
export interface UtilityAnswer {
  value: string;
  exact?: string;
  interpretation?: string;
  accuracy?: { significantDigits: number; rounded: boolean };
}
export function parseInput(query: string): UtilityRequest {
  const trimmed = query.trim();
  if (trimmed.startsWith('=')) return { kind: 'calculator', input: trimmed.slice(1).trim() };
  const match =
    /^(calc|계산|unit|변환|web|google|bing|ddg|namu|wiki|clip|clipboard|클립보드|dict|사전):\s*(.*)$/is.exec(
      trimmed,
    );
  if (!match) return { kind: 'files', input: trimmed };
  const prefix = match[1].toLowerCase();
  if (prefix === 'dict' || prefix === '사전') {
    const language = /^(ko|en|zh|ja)(?:\s+|:|$)\s*(.*)$/is.exec(match[2]);
    return {
      kind: 'dictionary',
      input: language ? language[2] : match[2],
      ...(language
        ? { dictionaryLanguage: language[1].toLowerCase() as UtilityRequest['dictionaryLanguage'] }
        : {}),
    };
  }
  const kind = ['clip', 'clipboard', '클립보드'].includes(prefix)
    ? 'clipboard'
    : ['calc', '계산'].includes(prefix)
      ? 'calculator'
      : ['unit', '변환'].includes(prefix)
        ? 'units'
        : 'web';
  return {
    kind,
    input: match[2],
    ...(kind === 'web' ? { provider: (prefix === 'web' ? 'google' : prefix) as WebProvider } : {}),
  };
}
export const providerNames: Record<WebProvider, string> = {
  google: 'Google',
  bing: 'Bing',
  ddg: 'DuckDuckGo',
  namu: '나무위키',
  wiki: 'Wikipedia',
};
export function webSearchUrl(provider: WebProvider, query: string) {
  const base = {
    google: 'https://www.google.com/search',
    bing: 'https://www.bing.com/search',
    ddg: 'https://duckduckgo.com/',
    namu: 'https://namu.wiki/Search',
    wiki: 'https://en.wikipedia.org/w/index.php',
  }[provider];
  const url = new URL(base);
  url.searchParams.set(provider === 'wiki' ? 'search' : 'q', query.trim());
  return url.href;
}
