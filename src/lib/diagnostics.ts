import { invoke } from '@tauri-apps/api/core';
import { bridge, native } from './bridge';

export interface DiagnosticSnapshot {
  schemaVersion: number;
  generatedAt: string;
  app: {
    version: string;
    os: string;
    architecture: string;
    development: boolean;
    distribution: 'installed' | 'portable' | 'packaged' | 'isolated';
  };
  settings: {
    filenameScopeCount: number;
    contentScopeCount: number;
    externalScopeCount: number;
    generationMode: 'local' | 'external' | 'auto';
    fallbackEnabled: boolean;
  } | null;
  health: {
    settingsError: boolean | null;
    shortcutError: boolean | null;
    startupError: boolean | null;
    dictionaryInitialized: boolean;
    embeddingInitialized: boolean;
    contentInitialized: boolean;
    chatInitialized: boolean;
    documentsInitialized: boolean;
    updatesInitialized: boolean;
  };
  filenameIndex: {
    scanning: boolean;
    count: number;
    skipped: number;
    warningCount: number;
    watching: boolean;
    /** Last validated NTFS snapshot only; callback time overlaps validatedFetchMs.
     * Excludes watcher setup, the final sweep and GUI rendering. */
    lastServiceSnapshot?: {
      receivedRecords: number;
      firstBatchMs: number | null;
      validatedFetchMs: number;
      receiveCallbackMs: number;
      finishPublishMs: number;
      totalMs: number;
      initialCatalog: boolean;
    } | null;
  } | null;
  contentIndex: {
    phase: string;
    discovered: number;
    indexed: number;
    skipped: number;
    embedded: number;
    issueCount: number;
    embeddingError: boolean;
  } | null;
  embedding: {
    phase: string;
    modelReady: boolean;
    runtimeReady: boolean;
    dimensions: number;
    error: boolean;
  } | null;
  clipboard: {
    enabled: boolean;
    paused: boolean;
    count: number;
    bytes: number;
    error: boolean;
  } | null;
  updates: {
    autoCheck: boolean;
    checking: boolean;
    installing: boolean;
    canInstall: boolean;
    checkError: boolean;
    installError: boolean;
    stateError: boolean;
  } | null;
}

export const MAX_DESCRIPTION_LENGTH = 4000;
export function diagnosticReport(snapshot: DiagnosticSnapshot, description: string) {
  // This exact plain-text document is previewed, copied and exported. User notes
  // are intentionally included only after the user types them in this panel.
  return `# Prism problem report\n\n## What happened / steps to reproduce\n\n${description.trim()}\n\n## Diagnostic snapshot\n\nService initialization flags do not imply that every operation is healthy.\nNull means unavailable or not read. Counts and status can change after capture.\n\n\x60\x60\x60json\n${JSON.stringify(snapshot, null, 2)}\n\x60\x60\x60\n`;
}

export const diagnostics = {
  async capture(): Promise<DiagnosticSnapshot> {
    if (!native) throw new Error('NATIVE_ONLY');
    return invoke('diagnostics_snapshot');
  },
  copy: (report: string) => bridge.copyText(report),
  save: (report: string) => bridge.exportMarkdown('Prism-problem-report', report),
  async openIssue() {
    if (native) await invoke('diagnostics_open_issue');
    else window.open('https://github.com/gaon12/prism/issues/new', '_blank', 'noopener,noreferrer');
  },
};
