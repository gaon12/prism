import { invoke } from '@tauri-apps/api/core';
import { credentialBackend, type CredentialBackend, type GenerationSlot } from './generation';
export interface RagFailure {
  code: string;
  message: string;
}
export interface ConversationSummary {
  id: string;
  title: string;
  createdMs: number;
  updatedMs: number;
  turnCount: number;
}
export interface SourceRef {
  chunkId: string;
  scopeId: string;
  filePath: string;
  sha256: string;
  page: number | null;
  paragraph: number | null;
}
export type SourceState = 'current' | 'changed' | 'deleted' | 'outOfScope';
export interface Citation {
  label: string;
  source: SourceRef;
  state: SourceState;
  isCited: boolean;
}
export interface ConversationTurn {
  id: string;
  conversationId: string;
  requestId: string;
  sequence: number;
  role: 'user' | 'assistant';
  text: string;
  createdMs: number;
  status: 'pending' | 'complete' | 'cancelled' | 'failed' | 'noSources' | 'interrupted';
  sourcesHidden: boolean;
  citations: Citation[];
  provenance: SourceRef[];
  taintComplete: boolean;
  evidence: 'noSources' | 'unverified' | 'sourceLinked';
  claimsVerified: false;
  unknownCitations: string[];
  provider: GenerationSlot | null;
  requestedModel: string | null;
  reportedModel: string | null;
  finishReason: string | null;
  usage: Record<string, unknown> | null;
  error: RagFailure | null;
}
export interface ConversationPage {
  conversation: ConversationSummary;
  turns: ConversationTurn[];
  hasMore: boolean;
}
export interface RagRequest {
  requestId: string;
  conversationId: string;
  question: string;
  scopeIds: string[];
  mode: 'documents' | 'general';
  sourcesHidden: boolean;
}
export interface RagAnswer {
  conversationId: string;
  requestId: string;
  userTurn: ConversationTurn;
  assistantTurn: ConversationTurn;
  attempts: { provider: GenerationSlot; error: RagFailure | null }[];
  omittedHistoryPairs: number;
}
export interface RagJob {
  requestId: string;
  conversationId: string;
  phase: string;
  provider: GenerationSlot | null;
  error: RagFailure | null;
}
interface CredentialStatus {
  hasKey: boolean;
  support: { supported: boolean; provider: string; userBound: boolean };
}
export const generationCredentials = {
  status: async (
    slot: GenerationSlot,
    endpoint: string,
  ): Promise<{ hasKey: boolean; support: CredentialBackend }> => {
    const status = await invoke<CredentialStatus>('generation_credential_status', {
      slot,
      endpoint,
    });
    return {
      hasKey: status.hasKey,
      support: credentialBackend(status.support),
    };
  },
  set: (slot: GenerationSlot, endpoint: string, key: string) =>
    invoke<void>('generation_set_key', { slot, endpoint, key }),
  clear: (slot: GenerationSlot, endpoint: string) =>
    invoke<void>('generation_clear_key', { slot, endpoint }),
};
export const rag = {
  create: (title: string | null = null) => invoke<ConversationSummary>('rag_create', { title }),
  list: (query = '', limit = 100) => invoke<ConversationSummary[]>('rag_list', { query, limit }),
  rename: (id: string, title: string) => invoke<void>('rag_rename', { id, title }),
  remove: (id: string) => invoke<void>('rag_delete', { id }),
  conversation: (id: string, beforeSequence: number | null = null, limit = 100) =>
    invoke<ConversationPage>('rag_conversation', { id, beforeSequence, limit }),
  ask: (request: RagRequest) => invoke<RagAnswer>('rag_ask', { request }),
  cancel: (conversationId: string, requestId: string) =>
    invoke<void>('rag_cancel', { conversationId, requestId }),
  job: (conversationId: string) => invoke<RagJob | null>('rag_job', { conversationId }),
  export: (id: string, includeSources = true) =>
    invoke<string>('rag_export', { id, includeSources }),
  sourcesHidden: (turnId: string, hidden: boolean) =>
    invoke<void>('rag_sources_hidden', { turnId, hidden }),
};
