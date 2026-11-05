import { describe, expect, it } from 'vitest';
import {
  defaultSettings,
  emptySession,
  isComposing,
  normalizeSession,
  normalizeSettings,
  rememberQuery,
  scopeName,
  scopePathKey,
} from './types';

describe('persisted client data', () => {
  it('migrates OCR as opt-in and rejects malformed language settings', () => {
    expect(normalizeSettings({}).ocr).toEqual({ enabled: false, language: 'auto', engine: 'auto' });
    expect(normalizeSettings({ ocr: { enabled: 'yes', language: '../../private' } }).ocr).toEqual({
      enabled: false,
      language: 'auto',
      engine: 'auto',
    });
    expect(normalizeSettings({ ocr: { enabled: true, language: 'ko' } }).ocr).toEqual({
      enabled: true,
      language: 'ko',
      engine: 'auto',
    });
    expect(normalizeSettings({ ocr: { enabled: true, engine: 'paddleVl16' } }).ocr.engine).toBe(
      'paddleVl16',
    );
    expect(normalizeSettings({ ocr: { engine: 'system' } }).ocr.engine).toBe('system');
    expect(normalizeSettings({ ocr: { engine: 'external' } }).ocr.engine).toBe('auto');
  });
  it('defaults to combined content retrieval and preserves an explicit opt-out', () => {
    expect(normalizeSettings({}).hybridSearch).toBe(true);
    expect(normalizeSettings({ hybridSearch: false }).hybridSearch).toBe(false);
  });
  it('distinguishes fresh settings from legacy profiles and preserves tour completion', () => {
    expect(normalizeSettings(undefined).onboardingVersion).toBe(0);
    expect(normalizeSettings({}).onboardingVersion).toBe(1);
    expect(normalizeSettings({ onboardingVersion: 0 }).onboardingVersion).toBe(0);
    expect(normalizeSettings({ onboardingVersion: 1 }).onboardingVersion).toBe(1);
  });
  it('keeps legacy search mode on filenames and preserves an explicit content mode', () => {
    expect(normalizeSession(undefined).mode).toBe('auto');
    expect(normalizeSession({ mode: 'auto' }).mode).toBe('auto');
    expect(normalizeSession({ query: '기획' }).mode).toBe('filename');
    expect(normalizeSession({ mode: 'content', query: '기획' }).mode).toBe('content');
    expect(normalizeSession({ mode: 'unknown', query: '기획' }).query).toBe('기획');
  });
  it('migrates old clipboard settings and preserves explicit off and unlimited retention', () => {
    expect(normalizeSettings({}).clipboard).toEqual(defaultSettings.clipboard);
    expect(
      normalizeSettings({
        clipboard: {
          enabled: false,
          captureImages: false,
          maxAgeDays: 0,
          maxBytes: Infinity,
          maxItems: -1,
          onboarded: true,
        },
      }).clipboard,
    ).toEqual({
      enabled: false,
      captureImages: false,
      maxAgeDays: 0,
      maxBytes: 256 * 1024 ** 2,
      maxItems: 10,
      onboarded: true,
    });
  });
  it('recovers malformed optional settings without rendering invalid controls', () => {
    const value = normalizeSettings({
      theme: 'unknown',
      language: null,
      fontSize: 200,
      shortcut: 123,
      rememberSearch: 'yes',
      scopes: [null, { id: 'docs', path: 'C:\\문서', contentEnabled: false }],
    });
    expect(value.theme).toBe('system');
    expect(value.language).toBe('ko');
    expect(value.fontSize).toBe(18);
    expect(value.shortcut).toBe('Alt+Space');
    expect(value.rememberSearch).toBe(true);
    expect(value.keyboardCorrection).toBe(true);
    expect(value.scopes).toEqual([
      {
        id: 'docs',
        name: '문서',
        path: 'C:\\문서',
        contentEnabled: false,
        filenameEnabled: true,
        externalAllowed: false,
      },
    ]);
  });
  it('keeps explicit false values and independent filename / AI scopes', () => {
    const settings = normalizeSettings({
      ...defaultSettings,
      rememberSearch: false,
      keyboardCorrection: false,
      startOnLogin: false,
      scopes: [
        { id: 'one', name: 'One', path: 'C:\\One', filenameEnabled: false, contentEnabled: true },
      ],
    });
    expect(settings.rememberSearch).toBe(false);
    expect(settings.keyboardCorrection).toBe(false);
    expect(settings.startOnLogin).toBe(false);
    expect(settings.scopes[0]).toMatchObject({ filenameEnabled: false, contentEnabled: true });
  });
  it('normalizes corrupt search history and preserves selection / filters', () => {
    expect(
      normalizeSession({
        query: 1,
        scopeId: null,
        recentQueries: ['문서', null, '', '문서'],
        extension: 'pdf',
        selectedPath: 'C:\\문서.pdf',
      }),
    ).toEqual({
      ...emptySession,
      mode: 'filename',
      recentQueries: ['문서'],
      extension: 'pdf',
      selectedPath: 'C:\\문서.pdf',
    });
  });
  it('keeps recent queries unique, trimmed and bounded', () => {
    const session = rememberQuery({
      ...emptySession,
      query: '  공문서  ',
      recentQueries: [
        '이전',
        '공문서',
        ...Array.from({ length: 25 }, (_, index) => `문서${index}`),
      ],
    });
    expect(session.recentQueries[0]).toBe('공문서');
    expect(session.recentQueries.filter((query) => query === '공문서')).toHaveLength(1);
    expect(session.recentQueries).toHaveLength(20);
    expect(rememberQuery({ ...emptySession, query: '   ' }).recentQueries).toEqual([]);
  });
});

describe('native path and keyboard boundaries', () => {
  it('deduplicates Windows slash / case variants while preserving POSIX case', () => {
    expect(scopePathKey('C:\\Documents\\')).toBe(scopePathKey('c:/documents'));
    expect(scopePathKey('\\\\NAS\\Shared')).toBe(scopePathKey('//nas/shared/'));
    expect(scopePathKey('/home/A')).not.toBe(scopePathKey('/home/a'));
    expect(scopeName('C:\\문서\\')).toBe('문서');
  });
  it('does not treat Korean IME confirmation as a search submission', () => {
    expect(isComposing({ isComposing: true })).toBe(true);
    expect(isComposing({ isComposing: false, keyCode: 229 })).toBe(true);
    expect(isComposing({ isComposing: false, keyCode: 13 })).toBe(false);
  });
});
