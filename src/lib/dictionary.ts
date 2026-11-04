import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { native } from './bridge';
import type { Language } from './types';

export type DictionaryLanguage = 'ko' | 'en' | 'zh' | 'ja';
export interface Definition {
  term: string;
  language: string;
  partOfSpeech: string;
  text: string;
  examples: string[];
  sourceUrl: string;
  sourceName: string;
  license: string;
  licenseUrl: string;
  definitionLanguage: string;
}
export interface DictionaryResponse {
  word: string;
  language: string;
  definitions: Definition[];
  source: 'online' | 'onlineCache' | 'offline';
  offlineReason: string | null;
  offlinePackInstalled: boolean;
}
export interface PackManifest {
  schemaVersion: number;
  language: DictionaryLanguage;
  name: string;
  version: string;
  sourceUrl: string;
  license: string;
  licenseUrl: string;
  definitionLanguage: string;
}
export interface InstalledPack {
  id: string;
  manifest: PackManifest;
  entries: number;
}
export interface DictionaryError {
  code: string;
  message: string;
}
export interface PackJob {
  phase: string;
  language: string | null;
  bytes: number;
  totalBytes: number | null;
  entries: number;
  error: DictionaryError | null;
}
export function dictionaryError(cause: unknown): DictionaryError {
  if (cause && typeof cause === 'object' && 'code' in cause && 'message' in cause)
    return { code: String(cause.code), message: String(cause.message) };
  return { code: 'worker', message: String(cause instanceof Error ? cause.message : cause) };
}
export const dictionary = {
  lookup: (language: DictionaryLanguage, word: string, offline: boolean) =>
    invoke<DictionaryResponse>('dictionary_lookup', { language, word, offline }),
  packs: () => invoke<InstalledPack[]>('dictionary_packs'),
  job: () => invoke<PackJob>('dictionary_job'),
  cancel: () => invoke<void>('dictionary_cancel'),
  download: (language: DictionaryLanguage) =>
    invoke<InstalledPack>('dictionary_download', { language }),
  remove: (id: string) => invoke<void>('dictionary_remove', { id }),
  openSource: (url: string) => invoke<void>('dictionary_open_source', { url }),
  async importPack() {
    const path = await open({
      multiple: false,
      filters: [{ name: 'Prism dictionary pack', extensions: ['jsonl', 'gz'] }],
    });
    return path ? invoke<InstalledPack>('dictionary_import', { path, manifest: null }) : null;
  },
};
export function guessDictionaryLanguage(word: string, ui: Language): DictionaryLanguage {
  if (/[ぁ-ゖァ-ヺ]/u.test(word)) return 'ja';
  if (/[가-힣ㄱ-ㅎㅏ-ㅣ]/u.test(word)) return 'ko';
  if (/\p{Script=Han}/u.test(word)) return 'zh';
  if (word.trim()) return 'en';
  return ui.startsWith('zh') ? 'zh' : ui === 'ja' ? 'ja' : ui === 'en' ? 'en' : 'ko';
}
export function useDictionary(
  active: boolean,
  word: string,
  requested: DictionaryLanguage | undefined,
  ui: Language,
) {
  const language = requested ?? guessDictionaryLanguage(word, ui);
  const key = JSON.stringify([language, word]);
  const current = useRef(key);
  current.current = key;
  const sequence = useRef(0);
  const inFlight = useRef<{ key: string; offline: boolean; token: number } | null>(null);
  const [result, setResult] = useState<{ key: string; data: DictionaryResponse } | null>(null);
  const [failure, setFailure] = useState<{ key: string; error: DictionaryError } | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  // Only an explicit submit starts a lookup; typing never sends a word to the network.
  const lookup = async (offline = !navigator.onLine) => {
    if (!active || !native || !word.trim()) return;
    if (inFlight.current?.key === key && inFlight.current.offline === offline) return;
    const token = ++sequence.current;
    inFlight.current = { key, offline, token };
    setPending(key);
    setFailure(null);
    setResult(null);
    try {
      const data = await dictionary.lookup(language, word, offline);
      if (current.current === key && inFlight.current?.token === token) setResult({ key, data });
    } catch (cause) {
      if (current.current === key && inFlight.current?.token === token)
        setFailure({ key, error: dictionaryError(cause) });
    } finally {
      if (inFlight.current?.token === token) {
        inFlight.current = null;
        setPending((previous) => (previous === key ? null : previous));
      }
    }
  };
  return {
    language,
    word,
    result: result?.key === key ? result.data : null,
    error: failure?.key === key ? failure.error : null,
    loading: pending === key,
    lookup,
  };
}
export type DictionaryController = ReturnType<typeof useDictionary>;
