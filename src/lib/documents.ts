import { invoke } from '@tauri-apps/api/core';
import type { Citation, RagFailure, RagJob, SourceRef } from './rag';
import type { GenerationSlot } from './generation';
export type DocumentBlockKind = 'fact' | 'userInput' | 'aiProposal' | 'blank';
export type DocumentOperation =
  'create' | 'outline' | 'outlineEdit' | 'body' | 'rewrite' | 'manual' | 'restore';
export interface PlanSection {
  id: string;
  heading: string;
  instruction: string;
}
export interface DocumentPlan {
  title: string;
  sections: PlanSection[];
}
export interface DocumentBlock {
  id: string;
  sectionId: string | null;
  kind: DocumentBlockKind;
  markdown: string;
  sourceLabels: string[];
  claimsVerified: boolean;
  userEdited: boolean;
}
export interface DocumentSummary {
  id: string;
  title: string;
  createdMs: number;
  updatedMs: number;
  currentRevisionId: string;
  revisionCount: number;
  hasReviewedOutline: boolean;
}
export interface DocumentRevision {
  id: string;
  documentId: string;
  sequence: number;
  parentRevisionId: string | null;
  createdMs: number;
  operation: DocumentOperation;
  plan: DocumentPlan;
  outlineReviewed: boolean;
  blocks: DocumentBlock[];
  markdown: string;
  scopeIds: string[];
  scopeSnapshot: {
    revision: number;
    selectedScopeIds: string[];
    scopes: {
      id: string;
      name: string;
      path: string;
      contentEnabled: boolean;
      externalAllowed: boolean;
    }[];
    externalQuestionsAllowed: boolean;
  };
  citations: Citation[];
  provenance: SourceRef[];
  externalInputEligible: boolean;
  ragRequestId: string | null;
  provider: GenerationSlot | null;
  requestedModel: string | null;
  reportedModel: string | null;
  usage: unknown | null;
  claimsVerified: boolean;
}
export interface DocumentPage {
  document: DocumentSummary;
  revision: DocumentRevision;
}
export interface DocumentRevisionSummary {
  id: string;
  sequence: number;
  parentRevisionId: string | null;
  createdMs: number;
  operation: DocumentOperation;
}
export interface DocumentFailure {
  code: string;
  message: string;
  rag: RagFailure | null;
}
export interface DocumentJob {
  documentId: string;
  requestId: string;
  phase: string;
  rag: RagJob | null;
  error: DocumentFailure | null;
}
export interface CreateDocument {
  title: string;
  purpose: string;
  scopeIds: string[];
  templateMarkdown: string | null;
}
export interface DocumentGenerateRequest {
  documentId: string;
  expectedRevisionId: string;
  requestId: string;
  instruction: string;
  scopeIds: string[];
}
export interface DocumentSelection {
  blockId: string;
  start: number;
  end: number;
}
export interface DocumentRewriteRequest extends DocumentGenerateRequest {
  targetBlockIds: string[];
  selection: DocumentSelection | null;
}
export const documents = {
  create: (input: CreateDocument) => invoke<DocumentPage>('doc_create', { input }),
  list: (query = '', limit = 100) => invoke<DocumentSummary[]>('doc_list', { query, limit }),
  get: (id: string, revisionId: string | null = null) =>
    invoke<DocumentPage>('doc_get', { id, revisionId }),
  revisions: (id: string, before: number | null = null, limit = 100) =>
    invoke<DocumentRevisionSummary[]>('doc_revisions', { id, before, limit }),
  rename: (id: string, value: string) => invoke<void>('doc_rename', { id, value }),
  remove: (id: string) => invoke<void>('doc_delete', { id }),
  saveOutline: (id: string, expected: string, plan: DocumentPlan, reviewed: boolean) =>
    invoke<DocumentRevision>('doc_save_outline', { id, expected, plan, reviewed }),
  saveManual: (
    id: string,
    expected: string,
    blocks: DocumentBlock[],
    plan: DocumentPlan | null = null,
  ) => invoke<DocumentRevision>('doc_save_manual', { id, expected, blocks, plan }),
  restore: (id: string, expected: string, target: string) =>
    invoke<DocumentRevision>('doc_restore', { id, expected, target }),
  export: (
    id: string,
    revision: string | null,
    includeSources: boolean,
    includeTypeMarkers: boolean,
  ) => invoke<string>('doc_export', { id, revision, includeSources, includeTypeMarkers }),
  generateOutline: (request: DocumentGenerateRequest) =>
    invoke<DocumentRevision>('doc_generate_outline', { request }),
  generateBody: (request: DocumentGenerateRequest) =>
    invoke<DocumentRevision>('doc_generate_body', { request }),
  rewrite: (request: DocumentRewriteRequest) =>
    invoke<DocumentRevision>('doc_rewrite', { request }),
  cancel: (id: string, request: string) => invoke<void>('doc_cancel', { id, request }),
  job: (id: string) => invoke<DocumentJob | null>('doc_job', { id }),
  currentCitations: (id: string, revisionId: string | null = null) =>
    invoke<Citation[]>('doc_current_citations', { id, revisionId }),
};

/** Reject UTF-16 positions inside surrogate pairs; Rust selection offsets count scalars. */
export function scalarSelection(
  text: string,
  start: number,
  end: number,
): { start: number; end: number } | null {
  const boundary = (at: number) =>
    Number.isInteger(at) &&
    at >= 0 &&
    at <= text.length &&
    !(
      at > 0 &&
      at < text.length &&
      /[\uD800-\uDBFF]/u.test(text[at - 1]) &&
      /[\uDC00-\uDFFF]/u.test(text[at])
    );
  if (!boundary(start) || !boundary(end) || start >= end) return null;
  return {
    start: Array.from(text.slice(0, start)).length,
    end: Array.from(text.slice(0, end)).length,
  };
}
