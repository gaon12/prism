import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
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
  /** The project (library folder / scope id) the conversation belongs to, if any. */
  project: string | null;
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
  selectionSet: SelectionPage | null;
  selection: ChatSelection | null;
  conversation: ConversationSummary;
  turns: ConversationTurn[];
  hasMore: boolean;
}
export interface ChatSelection {
  filenameSearch?: FilenameSearch;
  keywordSearch?: KeywordSearch;
  query: string;
  documents: string[];
  selectedDocuments: string[];
  partial: boolean;
}
export interface FilenameSearch {
  query: string;
  extension: string;
  scopeId: string | null;
  indexRevision: number;
  settingsRevision: number;
}
export interface KeywordSearch {
  query: string;
  extension: string;
  scopeId: string | null;
  contentRevision: number;
  settingsRevision: number;
}
export interface SelectionPage {
  query: string;
  partial: boolean;
  revision: number;
  total: number;
  selectedCount: number;
  offset: number;
  hasMore: boolean;
  files: { id: number; path: string; selected: boolean }[];
}
/** A transcript refresh must not reset the file page or replay older selection data. */
export function preferSelectionPage(current: SelectionPage | null, incoming: SelectionPage | null) {
  return current && incoming && current.revision >= incoming.revision ? current : incoming;
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
  createSearch: (selection: ChatSelection, requestId: string) =>
    selection.filenameSearch
      ? rag.createFilenameSearch(selection.filenameSearch, requestId)
      : selection.keywordSearch
        ? invoke<ConversationSummary>('rag_create_keyword_search', {
            search: selection.keywordSearch,
            requestId,
          })
        : rag.createFromSearch(selection),
  openMainChat: (id: string) => invoke<void>('rag_open_main_chat', { id }),
  takeChatHandoff: () => invoke<string | null>('rag_take_chat_handoff'),
  subscribeChatHandoff: (wake: () => void) => listen('prism:chat-handoff', wake),
  createFilenameSearch: (search: FilenameSearch, requestId: string) =>
    invoke<ConversationSummary>('rag_create_filename_search', { search, requestId }),
  fileSetPage: (id: string, offset: number) =>
    invoke<SelectionPage | null>('rag_file_set_page', { id, offset }),
  editFileSet: (
    id: string,
    revision: number,
    changes: [number, boolean][],
    all: boolean | null = null,
    clear = false,
  ) => invoke<void>('rag_edit_file_set', { id, revision, changes, all, clear }),
  createFromSearch: (selection: ChatSelection) =>
    invoke<ConversationSummary>('rag_create_from_search', { selection }),
  setSelection: (id: string, selection: ChatSelection | null) =>
    invoke<void>('rag_set_selection', { id, selection }),
  /** New conversation, filed under `project` when a library folder is open. */
  create: (title: string | null = null, project: string | null = null) =>
    invoke<ConversationSummary>('rag_create', { title, project }),
  /** With `project`, only that project's conversations; otherwise all of them. */
  list: (query = '', limit = 100, project: string | null = null) =>
    invoke<ConversationSummary[]>('rag_list', { query, limit, project }),
  /** Files a conversation that has no project yet under `project` (never moves one). */
  adoptProject: (id: string, project: string) => invoke<void>('rag_adopt_project', { id, project }),
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
