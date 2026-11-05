export type GenerationSlot = 'local' | 'external';
export type CredentialBackend =
  'windowsDpapi' | 'macosKeychain' | 'linuxSecretService' | 'unsupported';

export function credentialBackend(support: {
  supported: boolean;
  provider: string;
}): CredentialBackend {
  if (!support.supported) return 'unsupported';
  switch (support.provider) {
    case 'windowsDpapi':
      return 'windowsDpapi';
    case 'macos-keychain':
      return 'macosKeychain';
    case 'linux-secret-service':
      return 'linuxSecretService';
    default:
      return 'unsupported';
  }
}
export interface GenerationConfig {
  endpoint: string;
  model: string;
  maxTokens: number;
  tokenParameter: 'maxTokens' | 'maxCompletionTokens';
  temperature: number | null;
}
export interface GenerationPreferences {
  retrieval: 'hybrid' | 'semantic' | 'keyword';
  mode: GenerationSlot | 'auto';
  priority: 'localFirst' | 'externalFirst';
  fallback: boolean;
  allowExternalQuestions: boolean;
  maxContextChars: number | null;
  topK: number | null;
  local: GenerationConfig | null;
  external: GenerationConfig | null;
}
export const defaultGeneration: GenerationPreferences = {
  retrieval: 'hybrid',
  mode: 'local',
  priority: 'localFirst',
  fallback: false,
  allowExternalQuestions: false,
  maxContextChars: null,
  topK: null,
  local: null,
  external: null,
};
export function normalizeGeneration(input: unknown): GenerationPreferences {
  const value = input && typeof input === 'object' ? (input as Partial<GenerationPreferences>) : {};
  const number = (input: unknown, fallback: number, min: number, max: number) =>
    typeof input === 'number' && Number.isFinite(input)
      ? Math.min(max, Math.max(min, Math.round(input)))
      : fallback;
  const config = (input: GenerationConfig | null | undefined): GenerationConfig | null =>
    !input || typeof input !== 'object'
      ? null
      : {
          endpoint: typeof input.endpoint === 'string' ? input.endpoint : '',
          model: typeof input.model === 'string' ? input.model : '',
          maxTokens: number(input.maxTokens, 2048, 1, 65536),
          tokenParameter:
            input.tokenParameter === 'maxCompletionTokens' ? 'maxCompletionTokens' : 'maxTokens',
          temperature:
            typeof input.temperature === 'number' && Number.isFinite(input.temperature)
              ? Math.min(2, Math.max(0, input.temperature))
              : null,
        };
  return {
    retrieval:
      value.retrieval === 'semantic' || value.retrieval === 'keyword' ? value.retrieval : 'hybrid',
    mode: value.mode === 'external' || value.mode === 'auto' ? value.mode : 'local',
    priority: value.priority === 'externalFirst' ? 'externalFirst' : 'localFirst',
    fallback: value.fallback === true,
    allowExternalQuestions: value.allowExternalQuestions === true,
    maxContextChars:
      value.maxContextChars == null ? null : number(value.maxContextChars, 8000, 512, 200000),
    topK: value.topK == null ? null : number(value.topK, 6, 1, 32),
    local: config(value.local),
    external: config(value.external),
  };
}
