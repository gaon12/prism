import { describe, expect, it } from 'vitest';
import { credentialBackend, defaultGeneration, normalizeGeneration } from './generation';

it('permits only known available OS credential providers', () => {
  for (const [provider, expected] of [
    ['windowsDpapi', 'windowsDpapi'],
    ['macos-keychain', 'macosKeychain'],
    ['linux-secret-service', 'linuxSecretService'],
    ['unknown', 'unsupported'],
  ]) {
    expect(credentialBackend({ supported: true, provider })).toBe(expected);
    expect(credentialBackend({ supported: false, provider })).toBe('unsupported');
  }
});
describe('generation preferences migration', () => {
  it('requires an explicit keyless local choice and preserves legacy key lookup', () => {
    for (const localWithoutApiKey of [undefined, false, 'true', 1])
      expect(normalizeGeneration({ localWithoutApiKey }).localWithoutApiKey).toBe(false);
    expect(normalizeGeneration({ localWithoutApiKey: true }).localWithoutApiKey).toBe(true);
  });
  it('keeps old profiles local and blocks external fallback until explicitly allowed', () => {
    expect(normalizeGeneration(undefined)).toEqual(defaultGeneration);
    expect(
      normalizeGeneration({ mode: 'auto', fallback: 'true', allowExternalQuestions: 1 }),
    ).toMatchObject({ mode: 'auto', fallback: false, allowExternalQuestions: false });
  });
  it('keeps endpoint configuration but does not retain a pasted credential field', () => {
    const prefs = normalizeGeneration({
      external: {
        endpoint: 'https://example.org/v1',
        model: 'chosen-model',
        maxTokens: 2048,
        tokenParameter: 'maxCompletionTokens',
        temperature: null,
        apiKey: 'synthetic-must-not-persist',
      },
    });
    expect(prefs.external).toMatchObject({
      model: 'chosen-model',
      tokenParameter: 'maxCompletionTokens',
      temperature: null,
    });
    expect(JSON.stringify(prefs)).not.toContain('synthetic-must-not-persist');
  });
  it('keeps nullable defaults and bounds worker budgets independently', () => {
    expect(normalizeGeneration({ maxContextChars: null, topK: null })).toMatchObject({
      maxContextChars: null,
      topK: null,
    });
    expect(normalizeGeneration({ maxContextChars: 999999, topK: 999 })).toMatchObject({
      maxContextChars: 200000,
      topK: 32,
    });
  });
});

it('migrates retrieval independently of providers and accepts explicit search modes', () => {
  expect(normalizeGeneration({})).toMatchObject({ retrieval: 'hybrid', mode: 'local' });
  for (const retrieval of ['keyword', 'semantic', 'hybrid']) {
    expect(normalizeGeneration({ retrieval }).retrieval).toBe(retrieval);
  }
  expect(normalizeGeneration({ retrieval: 'unknown' }).retrieval).toBe('hybrid');
});
