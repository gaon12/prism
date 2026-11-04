import { describe, expect, it } from 'vitest';
import { defaultGeneration, normalizeGeneration } from './generation';
describe('generation preferences migration', () => {
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
