import { useEffect, useId, useRef, useState } from 'react';
import { Download, FileText, FolderSearch, Plus, RefreshCw, Square, Settings2 } from 'lucide-react';
import { bridge, native } from '../../lib/bridge';
import { documents, scalarSelection } from '../../lib/documents';
import type {
  DocumentBlock,
  DocumentBlockKind,
  DocumentFailure,
  DocumentGenerateRequest,
  DocumentJob,
  DocumentPage,
  DocumentPlan,
  DocumentRevision,
  DocumentRevisionSummary,
  DocumentSelection,
  DocumentSummary,
} from '../../lib/documents';
import type { Citation } from '../../lib/rag';
import { useClient } from '../../lib/store';
import type { Language, SearchScope } from '../../lib/types';
import { documentText } from './messages';
import type { DocumentMessage } from './messages';
import './documents.css';
import { FormFillPanel } from './FormFillPanel';
import {
  DocumentResearchSources,
  ExternalResearchInput,
  validResearchQuery,
} from './ExternalResearch';
import { researchText } from './research-messages';
import { AssemblyPlanControls, AssemblySectionEditor } from './AssemblyPlanEditor';
import { alignAssembly, assemblyPlanValid } from './assembly-plan';
const bytes = (value: string) => new TextEncoder().encode(value).length;
const titleValid = (value: string) =>
  Boolean(value.trim() && Array.from(value).length <= 200 && !/[\u0000-\u001f\u007f]/u.test(value));
const textValid = (value: string, max: number) => bytes(value) <= max && !value.includes('\0');
const planValid = (plan: DocumentPlan) =>
  titleValid(plan.title) &&
  plan.sections.length > 0 &&
  plan.sections.length <= 64 &&
  assemblyPlanValid(plan) &&
  plan.sections.every(
    (section) =>
      section.heading.trim() &&
      textValid(section.heading, 1200) &&
      textValid(section.instruction, 8000),
  );
const aiActive = (job: DocumentJob | null) =>
  Boolean(job && ['preparing', 'outline', 'body', 'rewrite'].includes(job.phase));
const docCodes = new Set([
  'input',
  'notFound',
  'conflict',
  'busy',
  'outlineReviewRequired',
  'malformed',
  'limit',
  'storage',
  'cancelled',
  'rag',
  'pdf',
  'hwpx',
  'unsupported',
  'timedOut',
  'externalRevoked',
]);
const ragCodes = new Set([
  'input',
  'storage',
  'busy',
  'duplicateRequest',
  'notConfigured',
  'noSources',
  'scopeBlocked',
  'policyChanged',
  'sourceChanged',
  'cancelled',
  'network',
  'auth',
  'rateLimited',
  'context',
  'server',
  'malformed',
  'endpoint',
  'worker',
  'unsupported',
  'limit',
]);
function failure(cause: unknown): DocumentFailure {
  const record = cause && typeof cause === 'object' ? (cause as Record<string, unknown>) : {};
  const inner =
    record.rag && typeof record.rag === 'object' ? (record.rag as Record<string, unknown>) : {};
  return {
    code: typeof record.code === 'string' && docCodes.has(record.code) ? record.code : 'storage',
    message: '',
    rag:
      typeof inner.code === 'string' && ragCodes.has(inner.code)
        ? { code: inner.code, message: '' }
        : null,
  };
}
function Failure({
  error,
  language,
  onModels,
}: {
  error: DocumentFailure;
  language: Language;
  onModels: () => void;
}) {
  const config = Boolean(
    error.rag && ['notConfigured', 'auth', 'endpoint', 'unsupported'].includes(error.rag.code),
  );
  const blocked = Boolean(
    error.code === 'externalRevoked' ||
    (error.rag && ['scopeBlocked', 'policyChanged', 'sourceChanged'].includes(error.rag.code)),
  );
  const key: DocumentMessage =
    error.code === 'externalRevoked'
      ? 'externalRevoked'
      : error.code === 'pdf'
        ? 'pdfError'
        : error.code === 'hwpx'
          ? 'hwpxError'
          : error.code === 'unsupported'
            ? 'hwpxUnsupported'
            : error.code === 'timedOut'
              ? 'exportTimedOut'
              : config
                ? 'settingsError'
                : blocked
                  ? 'blocked'
                  : error.code === 'conflict'
                    ? 'conflict'
                    : error.code === 'outlineReviewRequired'
                      ? 'reviewRequired'
                      : error.code === 'cancelled'
                        ? 'cancelled'
                        : error.code === 'input' || error.code === 'limit'
                          ? 'invalid'
                          : 'error';
  return (
    <div className="doc-error" role="alert">
      <p>
        {documentText(language, key)}{' '}
        <code>
          {error.code}
          {error.rag ? `/${error.rag.code}` : ''}
        </code>
      </p>
      {config ? (
        <button type="button" className="text-button" onClick={onModels}>
          {documentText(language, 'models')}
        </button>
      ) : null}
    </div>
  );
}
function ScopePicker({
  scopes,
  selected,
  onChange,
  language,
  onScopes,
}: {
  scopes: SearchScope[];
  selected: string[];
  onChange: (ids: string[]) => void;
  language: Language;
  onScopes: () => void;
}) {
  const current = selected.filter((id) => scopes.some((scope) => scope.id === id));
  return (
    <fieldset className="doc-scopes">
      <legend>{documentText(language, 'scopesSelect')}</legend>
      {scopes.map((scope) => (
        <label key={scope.id} className="doc-check">
          <input
            type="checkbox"
            checked={current.includes(scope.id)}
            onChange={(event) =>
              onChange(
                event.target.checked
                  ? [...current, scope.id]
                  : current.filter((id) => id !== scope.id),
              )
            }
          />
          <span>
            {scope.name}
            <small>{scope.path}</small>
            <small>
              {documentText(language, scope.externalAllowed ? 'externalYes' : 'externalNo')}
            </small>
          </span>
        </label>
      ))}
      <p className="doc-note">{documentText(language, 'scopeHelp')}</p>
      {!current.length ? (
        <p className="doc-warning" role="status">
          {documentText(language, 'noScopes')}
        </p>
      ) : null}
      <button type="button" className="text-button" onClick={onScopes}>
        {documentText(language, 'scopes')}
      </button>
    </fieldset>
  );
}
function SourceRecords({
  revision,
  policyKey,
  language,
}: {
  revision: DocumentRevision;
  policyKey: string;
  language: Language;
}) {
  const [hidden, setHidden] = useState(false);
  const [state, setState] = useState<{ key: string; items: Citation[] } | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const key = JSON.stringify([revision.id, policyKey]);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(false);
    void documents
      .currentCitations(revision.documentId, revision.id)
      .then((items) => {
        if (active) setState({ key, items });
      })
      .catch(() => {
        if (active) setError(true);
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [key, revision.id, revision.documentId, refresh]);
  const current = !loading && !error && state?.key === key ? state.items : [];
  const t = (name: DocumentMessage) => documentText(language, name);
  const unknown = [
    ...new Set(
      revision.blocks
        .flatMap((block) => block.sourceLabels)
        .filter(
          (label) =>
            !revision.citations.some((cite) => label === cite.label || label === `[${cite.label}]`),
        ),
    ),
  ];
  return (
    <details className="doc-source-records">
      <summary>{t('citations')}</summary>
      <header>
        <h3>{t('citations')}</h3>
        <button
          type="button"
          className="icon-button small"
          title={t('refresh')}
          aria-label={t('refresh')}
          disabled={loading}
          onClick={() => setRefresh((value) => value + 1)}
        >
          <RefreshCw size={14} />
        </button>
      </header>
      <p className="doc-note">{t('citationHelp')}</p>
      <label className="doc-check">
        <input
          type="checkbox"
          checked={hidden}
          onChange={(event) => setHidden(event.target.checked)}
        />
        <span>{t('hideSources')}</span>
      </label>
      <p className="doc-note">{t('hiddenHelp')}</p>
      {!hidden ? (
        <>
          {loading ? (
            <p role="status" className="doc-note">
              {t('loading')}
            </p>
          ) : null}
          {error ? (
            <p role="alert" className="doc-warning">
              {t('sourceUnknown')}
            </p>
          ) : null}
          {!revision.citations.length ? (
            <p className="doc-note">{t('noCitations')}</p>
          ) : (
            <ul>
              {revision.citations.map((citation) => {
                const now = current.find(
                  (item) =>
                    item.label === citation.label &&
                    item.source.chunkId === citation.source.chunkId &&
                    item.source.sha256 === citation.source.sha256,
                );
                const status: DocumentMessage = now
                  ? now.state === 'current'
                    ? 'sourceCurrent'
                    : now.state === 'changed'
                      ? 'sourceChanged'
                      : now.state === 'deleted'
                        ? 'sourceDeleted'
                        : 'sourceOutOfScope'
                  : 'sourceUnknown';
                return (
                  <li key={`${citation.label}:${citation.source.chunkId}`}>
                    <strong>[{citation.label}]</strong>
                    <span className="doc-source-status">{t(status)}</span>
                    <span className="doc-source-file" title={citation.source.filePath}>
                      {citation.source.filePath.split(/[\\/]/u).pop()}
                    </span>
                    <details>
                      <summary>{t('sourceDetails')}</summary>
                      <code>{citation.source.filePath}</code>
                      <code>
                        {citation.source.scopeId.startsWith('mcp_')
                          ? researchText(language, 'fingerprint')
                          : 'SHA-256'}{' '}
                        {citation.source.sha256}
                      </code>
                    </details>
                    <small>
                      {citation.source.page !== null
                        ? documentText(language, 'page', { value: citation.source.page })
                        : ''}
                      {citation.source.paragraph !== null
                        ? ' · ' +
                          documentText(language, 'paragraph', { value: citation.source.paragraph })
                        : ''}
                    </small>
                  </li>
                );
              })}
            </ul>
          )}
          {unknown.map((label) => (
            <p key={label} className="doc-note">
              {documentText(language, 'unknownCitation', { value: label })}
            </p>
          ))}
          <DocumentResearchSources revision={revision} policyKey={policyKey} language={language} />
        </>
      ) : null}
    </details>
  );
}
interface EditorProps {
  id: string;
  language: Language;
  scopes: SearchScope[];
  scopeIds: string[];
  onScopeIds: (ids: string[]) => void;
  policyKey: string;
  externalQuestions: boolean;
  researchAvailable: boolean;
  onModels: () => void;
  onScopes: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  onDirty: (dirty: boolean) => void;
}
function DocumentEditor({
  id,
  language,
  scopes,
  scopeIds,
  onScopeIds,
  policyKey,
  externalQuestions,
  researchAvailable,
  onModels,
  onScopes,
  onChanged,
  onDeleted,
  onDirty,
}: EditorProps) {
  const fieldId = useId();
  const [pane, setPane] = useState<'outline' | 'body'>('outline');
  const [optionsOpen, setOptionsOpen] = useState(false);
  const outlineTab = useRef<HTMLButtonElement>(null);
  const bodyTab = useRef<HTMLButtonElement>(null);
  const t = (key: DocumentMessage) => documentText(language, key);
  const alive = useRef(false);
  const epoch = useRef(0);
  const editVersion = useRef(0);
  const syncPending = useRef(false);
  const aiPending = useRef(false);
  const activeRequest = useRef<string | null>(null);
  const completedRequest = useRef<string | null>(null);
  const exportRequest = useRef<{ request: string; format: 'pdf' | 'hwpx' } | null>(null);
  const [exportStopping, setExportStopping] = useState(false);
  const [exportRunning, setExportRunning] = useState(false);
  const [exportVersion, setExportVersion] = useState<number | null>(null);
  const jobEpoch = useRef(0);
  const current = useRef<DocumentPage | null>(null);
  const dirtyRef = useRef(false);
  const [page, setPage] = useState<DocumentPage | null>(null);
  const [researchChoice, setResearchChoice] = useState({ policyKey, enabled: false });
  const researchEnabled = researchChoice.policyKey === policyKey && researchChoice.enabled;
  const setResearchEnabled = (enabled: boolean) => setResearchChoice({ policyKey, enabled });
  const [researchQuery, setResearchQuery] = useState('');
  const [access, setAccess] = useState<{ key: string; error: DocumentFailure | null } | null>(null);
  const [accessRefresh, setAccessRefresh] = useState(0);
  const hasExternal = Boolean(
    page?.revision.provenance.some((source) => source.scopeId.startsWith('mcp_')),
  );
  const accessKey = JSON.stringify([id, page?.revision.id, policyKey]);
  const accessReady = !hasExternal || (access?.key === accessKey && !access.error);
  useEffect(() => {
    if (!page || !hasExternal) return;
    let active = true;
    void documents
      .validateAccess(id, page.revision.id)
      .then(() => {
        if (active) {
          setAccess({ key: accessKey, error: null });
          setError((previous) => (previous?.code === 'externalRevoked' ? null : previous));
        }
      })
      .catch((cause) => {
        if (active) setAccess({ key: accessKey, error: failure(cause) });
      });
    return () => {
      active = false;
    };
  }, [accessKey, hasExternal, id, page?.revision.id, accessRefresh]);
  current.current = page;
  const [plan, setPlan] = useState<DocumentPlan>({ title: '', sections: [] });
  const [blocks, setBlocks] = useState<DocumentBlock[]>([]);
  const [reviewed, setReviewed] = useState(false);
  const [incoming, setIncoming] = useState<DocumentPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<DocumentFailure | null>(null);
  const [notice, setNotice] = useState<DocumentMessage | null>(null);
  const [history, setHistory] = useState<DocumentRevisionSummary[]>([]);
  const [historyMore, setHistoryMore] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const historyPending = useRef(false);
  const historyEpoch = useRef(0);
  const scopesRef = useRef(scopes);
  scopesRef.current = scopes;
  const scopeInitialized = useRef(false);
  const [job, setJob] = useState<DocumentJob | null>(null);
  const [generating, setGenerating] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [action, setAction] = useState<string | null>(null);
  const [instruction, setInstruction] = useState('');
  const [selectedBlocks, setSelectedBlocks] = useState<string[]>([]);
  const [selection, setSelection] = useState<DocumentSelection | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const [exportSources, setExportSources] = useState(true);
  const [exportMarkers, setExportMarkers] = useState(true);
  const outlineDirty = Boolean(page && JSON.stringify(plan) !== JSON.stringify(page.revision.plan));
  const blocksDirty = Boolean(
    page && JSON.stringify(blocks) !== JSON.stringify(page.revision.blocks),
  );
  const dirty = outlineDirty || blocksDirty;
  dirtyRef.current = dirty;
  const historical = Boolean(page && page.revision.id !== page.document.currentRevisionId);
  const validScopes = scopeIds.filter((scopeId) => scopes.some((scope) => scope.id === scopeId));
  const busy = generating || aiActive(job);
  const editable = Boolean(page && !historical);
  const canAI =
    accessReady &&
    (!researchEnabled || (researchAvailable && validResearchQuery(researchQuery))) &&
    editable &&
    !dirty &&
    !incoming &&
    !loading &&
    !busy &&
    !action &&
    validScopes.length > 0 &&
    validScopes.length <= 128 &&
    textValid(instruction, 8192);
  const bodyAllowed = Boolean(canAI && page?.revision.outlineReviewed && plan.sections.length);
  const validBlocks =
    blocks.length <= 256 &&
    blocks.every(
      (block) => textValid(block.markdown, 131072) && (block.kind !== 'blank' || !block.markdown),
    ) &&
    blocks.reduce((total, block) => total + bytes(block.markdown), 0) <= 131072;
  const adopt = (next: DocumentPage) => {
    editVersion.current++;
    current.current = next;
    dirtyRef.current = false;
    setPage(next);
    setPlan(structuredClone(next.revision.plan));
    setBlocks(structuredClone(next.revision.blocks));
    setReviewed(false);
    setIncoming(null);
    setSelection(null);
    setSelectedBlocks([]);
    setNewTitle(next.document.title);
    if (!scopeInitialized.current) {
      scopeInitialized.current = true;
      onScopeIds(
        next.revision.scopeIds.filter((id) => scopesRef.current.some((scope) => scope.id === id)),
      );
    }
    setPane(next.revision.blocks.length || next.revision.outlineReviewed ? 'body' : 'outline');
  };
  const summaryWithRevision = (revision: DocumentRevision): DocumentPage => ({
    document: {
      ...current.current!.document,
      currentRevisionId: revision.id,
      revisionCount: Math.max(current.current!.document.revisionCount, revision.sequence),
      hasReviewedOutline: revision.outlineReviewed,
    },
    revision,
  });
  const receive = (next: DocumentPage, version: number, expected: string | null, force = false) => {
    if (!alive.current) return;
    if (current.current?.revision.id === next.revision.id) {
      setPage((previous) => (previous ? { ...previous, document: next.document } : next));
      return;
    }
    if (
      !force &&
      current.current &&
      next.revision.sequence < current.current.revision.sequence &&
      next.document.currentRevisionId === next.revision.id
    )
      return;
    if (editVersion.current === version && (!expected || current.current?.revision.id === expected))
      adopt(next);
    else setIncoming(next);
  };
  useEffect(() => {
    onDirty(dirty);
    return () => onDirty(false);
  }, [dirty, onDirty]);
  const fetchCurrent = async () => {
    const token = ++epoch.current;
    const version = editVersion.current;
    const wasDirty = dirtyRef.current;
    setLoading(true);
    setError(null);
    try {
      const next = await documents.get(id);
      if (alive.current && token === epoch.current) {
        if (wasDirty && current.current?.revision.id !== next.revision.id) setIncoming(next);
        else receive(next, version, current.current?.revision.id ?? null);
      }
    } catch (cause) {
      if (alive.current && token === epoch.current) setError(failure(cause));
    } finally {
      if (alive.current && token === epoch.current) setLoading(false);
    }
  };
  useEffect(() => {
    if (!current.current && error?.code === 'externalRevoked') void fetchCurrent();
  }, [policyKey]);
  const fetchHistory = async (older = false) => {
    if (older && historyPending.current) return;
    historyPending.current = true;
    setHistoryBusy(true);
    const token = ++historyEpoch.current;
    try {
      const before = older ? (history[history.length - 1]?.sequence ?? null) : null;
      const items = await documents.revisions(id, before, 100);
      if (alive.current && token === historyEpoch.current) {
        setHistory((previous) =>
          older
            ? [...previous, ...items.filter((item) => !previous.some((old) => old.id === item.id))]
            : items,
        );
        setHistoryMore(Boolean(items.length && items[items.length - 1].sequence > 1));
      }
    } catch (cause) {
      if (alive.current) setError(failure(cause));
    } finally {
      if (token === historyEpoch.current) {
        historyPending.current = false;
        if (alive.current) setHistoryBusy(false);
      }
    }
  };
  useEffect(() => {
    alive.current = true;
    void fetchCurrent();
    return () => {
      alive.current = false;
      epoch.current++;
      const pending = exportRequest.current;
      if (pending)
        void documents[pending.format === 'pdf' ? 'cancelPdf' : 'cancelHwpx'](
          pending.request,
        ).catch(() => {});
    };
  }, [id]);
  useEffect(() => {
    if (!page) return;
    void fetchHistory();
  }, [page?.document.currentRevisionId]);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let previous: string | null = null;
    const poll = async () => {
      const requestedAt = jobEpoch.current;
      try {
        const next = await documents.job(id);
        if (disposed || !alive.current) return;
        // A status read begun before a local operation started/completed is
        // older than that operation's response, even if it arrives later.
        if (requestedAt === jobEpoch.current) {
          setJob(next);
          const signature = next ? `${next.requestId}:${next.phase}` : 'none';
          // The successful generation response already applied this revision
          // (or retained it for dirty-input review). Re-reading it on a late
          // terminal poll would briefly disable actions through `loading`,
          // swallowing a click made while that redundant read is in flight.
          const ownCompletion =
            next?.phase === 'complete' && next.requestId === completedRequest.current;
          if (previous !== null && signature !== previous && !aiActive(next) && !ownCompletion) {
            void fetchCurrent();
            onChanged();
          }
          previous = signature;
        }
      } catch (cause) {
        if (!disposed && alive.current && requestedAt === jobEpoch.current)
          setError(failure(cause));
      }
      if (!disposed) timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [id]);
  const updatePlan = (next: DocumentPlan) => {
    editVersion.current++;
    setPlan(alignAssembly(next));
    setReviewed(false);
    setSelection(null);
  };
  const updateBlock = (blockId: string, patch: Partial<DocumentBlock>) => {
    editVersion.current++;
    setBlocks((previous) =>
      previous.map((block) =>
        block.id === blockId
          ? { ...block, ...patch, userEdited: true, claimsVerified: false }
          : block,
      ),
    );
    setSelection(null);
  };
  const commit = async (kind: 'outline' | 'manual' | 'restore', target?: string) => {
    if (
      !page ||
      syncPending.current ||
      loading ||
      (incoming && kind !== 'restore') ||
      (kind === 'outline' && (!editable || !reviewed || !planValid(plan) || blocksDirty)) ||
      (kind === 'manual' && (!editable || outlineDirty || !validBlocks)) ||
      (kind === 'restore' && (dirty || busy || aiPending.current || !target))
    )
      return;
    syncPending.current = true;
    setAction(kind);
    setError(null);
    setNotice(null);
    const version = editVersion.current;
    const expected = page.document.currentRevisionId;
    try {
      const revision =
        kind === 'outline'
          ? await documents.saveOutline(id, expected, plan, true)
          : kind === 'manual'
            ? await documents.saveManual(id, expected, blocks)
            : await documents.restore(id, expected, target!);
      if (alive.current) {
        receive(summaryWithRevision(revision), version, page.revision.id, true);
        setNotice('saved');
        onChanged();
      }
    } catch (cause) {
      if (alive.current) setError(failure(cause));
    } finally {
      syncPending.current = false;
      if (alive.current) setAction(null);
    }
  };
  const generate = async (
    kind: 'outline' | 'body' | 'rewrite',
    targets: string[] = [],
    range: DocumentSelection | null = null,
  ) => {
    if (
      !page ||
      !canAI ||
      syncPending.current ||
      aiPending.current ||
      (kind === 'body' && !bodyAllowed) ||
      (kind === 'rewrite' && !targets.length)
    )
      return;
    aiPending.current = true;
    jobEpoch.current++;
    const requestId = crypto.randomUUID();
    activeRequest.current = requestId;
    setGenerating(true);
    setStopping(false);
    setError(null);
    setNotice(null);
    const version = editVersion.current;
    const expected = page.revision.id;
    const request: DocumentGenerateRequest = {
      researchQuery: researchEnabled && researchAvailable ? researchQuery : null,
      documentId: id,
      expectedRevisionId: expected,
      requestId,
      instruction,
      scopeIds: [...validScopes],
    };
    setResearchEnabled(false);
    try {
      const revision =
        kind === 'outline'
          ? await documents.generateOutline(request)
          : kind === 'body'
            ? await documents.generateBody(request)
            : await documents.rewrite({ ...request, targetBlockIds: targets, selection: range });
      if (alive.current) {
        completedRequest.current = requestId;
        jobEpoch.current++;
        setJob((previous) =>
          previous?.requestId === requestId
            ? { ...previous, phase: 'complete', rag: null, error: null }
            : previous,
        );
        receive(summaryWithRevision(revision), version, expected);
        setNotice('saved');
        onChanged();
      }
    } catch (cause) {
      if (alive.current) setError(failure(cause));
    } finally {
      aiPending.current = false;
      activeRequest.current = null;
      if (alive.current) setGenerating(false);
    }
  };
  const stop = async () => {
    const request = activeRequest.current ?? (aiActive(job) ? job?.requestId : null);
    if (!request || stopping) return;
    setStopping(true);
    try {
      await documents.cancel(id, request);
    } catch (cause) {
      if (alive.current) {
        setError(failure(cause));
        setStopping(false);
      }
    }
  };
  const openRevision = async (revisionId: string) => {
    if (dirty || syncPending.current) return;
    const token = ++epoch.current;
    const version = editVersion.current;
    const expected = page?.revision.id ?? null;
    setLoading(true);
    setError(null);
    try {
      const next = await documents.get(id, revisionId);
      if (alive.current && token === epoch.current) receive(next, version, expected, true);
    } catch (cause) {
      if (alive.current && token === epoch.current) setError(failure(cause));
    } finally {
      if (alive.current && token === epoch.current) setLoading(false);
    }
  };
  const runAction = async (kind: 'rename' | 'delete' | 'export' | 'exportPdf' | 'exportHwpx') => {
    if (!page || syncPending.current || ((kind === 'rename' || kind === 'delete') && busy)) return;
    syncPending.current = true;
    setAction(kind);
    const fileExport = kind === 'exportPdf' || kind === 'exportHwpx';
    if (fileExport) setExportVersion(page.revision.sequence);
    setError(null);
    setNotice(null);
    try {
      if (kind === 'rename') {
        if (!titleValid(newTitle)) return;
        await documents.rename(id, newTitle);
        if (alive.current) {
          setPage((previous) =>
            previous
              ? { ...previous, document: { ...previous.document, title: newTitle } }
              : previous,
          );
          setRenaming(false);
          onChanged();
        }
      } else if (kind === 'delete') {
        await documents.remove(id);
        if (alive.current) onDeleted();
      } else if (kind === 'exportPdf' || kind === 'exportHwpx') {
        const isPdf = kind === 'exportPdf';
        const path = await bridge[isPdf ? 'choosePdfPath' : 'chooseHwpxPath'](page.document.title);
        if (!path || !alive.current) return;
        const request = crypto.randomUUID();
        exportRequest.current = { request, format: isPdf ? 'pdf' : 'hwpx' };
        setExportRunning(true);
        const result = await documents[isPdf ? 'exportPdf' : 'exportHwpx'](
          id,
          page.revision.id,
          exportSources,
          exportMarkers,
          request,
          path,
        );
        if (alive.current)
          setNotice(
            isPdf
              ? result.omittedImages
                ? 'pdfExportedOmitted'
                : 'pdfExported'
              : result.omittedImages || ('literalHtml' in result && result.literalHtml)
                ? 'hwpxExportedOmitted'
                : 'hwpxExported',
          );
      } else {
        const markdown = await documents.export(id, page.revision.id, exportSources, exportMarkers);
        if (!alive.current) return;
        if ((await bridge.exportMarkdown(page.document.title, markdown)) && alive.current)
          setNotice('exported');
      }
    } catch (cause) {
      if (alive.current) {
        const value = failure(cause);
        if (fileExport && value.code === 'cancelled')
          setNotice(kind === 'exportPdf' ? 'pdfCancelled' : 'hwpxCancelled');
        else setError(value);
      }
    } finally {
      syncPending.current = false;
      if (fileExport) exportRequest.current = null;
      if (alive.current) {
        setAction(null);
        setExportStopping(false);
        setExportRunning(false);
        setExportVersion(null);
      }
    }
  };
  if (!page)
    return (
      <div className="doc-editor">
        {loading ? <p role="status">{t('loading')}</p> : null}
        {error ? <Failure error={error} language={language} onModels={onModels} /> : null}
        <button
          type="button"
          className="button"
          disabled={loading}
          onClick={() => void fetchCurrent()}
        >
          {t('refresh')}
        </button>
      </div>
    );
  if (!accessReady)
    return (
      <div className="doc-editor doc-access-check">
        {access?.key === accessKey && access.error ? (
          <Failure error={access.error} language={language} onModels={onModels} />
        ) : (
          <p role="status">{t('validatingAccess')}</p>
        )}
        <button
          type="button"
          className="button"
          onClick={() => setAccessRefresh((value) => value + 1)}
        >
          {t('refresh')}
        </button>
        {busy ? (
          <button type="button" className="button" onClick={() => void stop()}>
            {t('stop')}
          </button>
        ) : null}
      </div>
    );
  const unknownLabels = (block: DocumentBlock) =>
    block.sourceLabels.filter(
      (label) =>
        !page.revision.citations.some(
          (citation) => label === citation.label || label === `[${citation.label}]`,
        ),
    );
  return (
    <section className="doc-editor" aria-label={page.document.title}>
      <header className="doc-editor-heading">
        <div>
          <h2>{page.document.title}</h2>
          <small>{documentText(language, 'version', { value: page.revision.sequence })}</small>
        </div>
        <div className="doc-actions">
          <button
            type="button"
            className="icon-button small"
            title={t('refresh')}
            aria-label={t('refresh')}
            disabled={loading}
            onClick={() => void fetchCurrent()}
          >
            <RefreshCw size={15} />
          </button>
          <button
            type="button"
            className="text-button"
            disabled={Boolean(action) || busy}
            onClick={() => setRenaming((value) => !value)}
          >
            {t('rename')}
          </button>
          <button
            type="button"
            className="text-button"
            disabled={Boolean(action) || busy}
            onClick={() => setDeleteConfirm((value) => !value)}
          >
            {t('remove')}
          </button>
        </div>
      </header>
      <div className="doc-primary-toolbar">
        <div
          className="doc-tabs"
          role="tablist"
          aria-label={t('title')}
          onKeyDown={(event) => {
            if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
              event.preventDefault();
              const next =
                event.key === 'Home'
                  ? 'outline'
                  : event.key === 'End'
                    ? 'body'
                    : pane === 'outline'
                      ? 'body'
                      : 'outline';
              setPane(next);
              (next === 'outline' ? outlineTab : bodyTab).current?.focus();
            }
          }}
        >
          <button
            type="button"
            role="tab"
            ref={outlineTab}
            id={`${fieldId}-outline-tab`}
            aria-selected={pane === 'outline'}
            aria-controls={`${fieldId}-outline-pane`}
            tabIndex={pane === 'outline' ? 0 : -1}
            onClick={() => setPane('outline')}
          >
            {t('outline')}
          </button>
          <button
            type="button"
            role="tab"
            ref={bodyTab}
            id={`${fieldId}-body-tab`}
            aria-selected={pane === 'body'}
            aria-controls={`${fieldId}-body-pane`}
            tabIndex={pane === 'body' ? 0 : -1}
            onClick={() => setPane('body')}
          >
            {t('body')}
          </button>
        </div>
        <div className="doc-actions">
          {pane === 'outline' ? (
            <>
              <button
                type="button"
                className="button"
                disabled={!canAI}
                onClick={() => void generate('outline')}
              >
                {t('generateOutline')}
              </button>
              <button
                type="button"
                className="button primary"
                disabled={
                  !editable ||
                  Boolean(incoming) ||
                  !reviewed ||
                  !planValid(plan) ||
                  blocksDirty ||
                  Boolean(action)
                }
                onClick={() => void commit('outline')}
              >
                {t('saveOutline')}
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                className="button"
                disabled={!bodyAllowed}
                onClick={() => void generate('body')}
              >
                {t('generateBody')}
              </button>
              <button
                type="button"
                className="button primary"
                disabled={
                  !editable ||
                  Boolean(incoming) ||
                  !blocksDirty ||
                  outlineDirty ||
                  !validBlocks ||
                  Boolean(action)
                }
                onClick={() => void commit('manual')}
              >
                {t('saveManual')}
              </button>
            </>
          )}
        </div>
      </div>
      {!validScopes.length || validScopes.length > 128 ? (
        <p className="doc-warning doc-compact-warning">
          {!validScopes.length ? t('noScopes') : t('scopeHelp')}{' '}
          <button type="button" className="text-button" onClick={() => setOptionsOpen(true)}>
            {t('generationOptions')}
          </button>
        </p>
      ) : null}
      {renaming ? (
        <div className="doc-inline">
          <label htmlFor={`${fieldId}-rename`}>{t('docTitle')}</label>
          <input
            id={`${fieldId}-rename`}
            value={newTitle}
            maxLength={400}
            onChange={(event) => setNewTitle(event.target.value)}
          />
          <button
            type="button"
            className="button"
            disabled={!titleValid(newTitle) || Boolean(action)}
            onClick={() => void runAction('rename')}
          >
            {t('saveName')}
          </button>
          <button type="button" className="text-button" onClick={() => setRenaming(false)}>
            {t('cancel')}
          </button>
        </div>
      ) : null}
      {deleteConfirm ? (
        <div className="doc-confirm">
          <p>{t('deleteConfirm')}</p>
          <button
            type="button"
            className="button"
            disabled={Boolean(action) || busy}
            onClick={() => void runAction('delete')}
          >
            {t('deleteNow')}
          </button>
          <button type="button" className="text-button" onClick={() => setDeleteConfirm(false)}>
            {t('cancel')}
          </button>
        </div>
      ) : null}
      {error ? <Failure error={error} language={language} onModels={onModels} /> : null}
      {notice ? (
        <p role="status" className="doc-note">
          {t(notice)}
        </p>
      ) : null}
      {incoming ? (
        <div className="doc-incoming" role="status">
          <p>{t('incoming')}</p>
          <button type="button" className="button" onClick={() => adopt(incoming)}>
            {t('applyIncoming')}
          </button>
        </div>
      ) : null}
      {historical ? (
        <p className="doc-warning">
          {t('historical')}{' '}
          <button type="button" className="text-button" onClick={() => void fetchCurrent()}>
            {t('current')}
          </button>
        </p>
      ) : null}
      {dirty ? (
        <div className="doc-unsaved">
          <p>{t('unsaved')}</p>
          <button type="button" className="text-button" onClick={() => adopt(incoming ?? page)}>
            {t('discard')}
          </button>
        </div>
      ) : null}
      <div className="doc-generation-status" role="status">
        {busy
          ? t('working')
          : job && ['complete', 'cancelled', 'failed', 'interrupted'].includes(job.phase)
            ? t(job.phase as 'complete' | 'cancelled' | 'failed' | 'interrupted')
            : null}
        {job?.rag?.phase
          ? ' · ' + t(job.rag.phase === 'retrieving' ? 'preparing' : 'working')
          : null}
        {busy ? (
          <button type="button" className="button" disabled={stopping} onClick={() => void stop()}>
            <Square size={14} />
            {t('stop')}
          </button>
        ) : null}
        {stopping && busy ? <small>{t('stopRequested')}</small> : null}
      </div>
      {job?.error ? (
        <Failure error={failure(job.error)} language={language} onModels={onModels} />
      ) : null}
      {pane === 'body' && (selectedBlocks.length || selection) ? (
        <div className="doc-selection-actions">
          <button
            type="button"
            className="button"
            disabled={!canAI || !selectedBlocks.length}
            onClick={() => void generate('rewrite', selectedBlocks)}
          >
            {t('rewriteSelected')}
          </button>
          <button
            type="button"
            className="button"
            disabled={!canAI || !selection}
            onClick={() => {
              if (selection) void generate('rewrite', [selection.blockId], selection);
            }}
          >
            {t('rewriteSelection')}
          </button>
          {selection ? (
            <small>
              {documentText(language, 'selectedRange', {
                start: selection.start,
                end: selection.end,
              })}
            </small>
          ) : null}
        </div>
      ) : null}
      <div className="doc-edit-scroll">
        <details
          className="doc-options"
          open={optionsOpen}
          onToggle={(event) => setOptionsOpen(event.currentTarget.open)}
        >
          <summary>
            {t('generationOptions')} · {validScopes.length}
          </summary>
          <ScopePicker
            scopes={scopes}
            selected={validScopes}
            onChange={onScopeIds}
            language={language}
            onScopes={onScopes}
          />
          <p className="doc-note">{t(externalQuestions ? 'questionsYes' : 'questionsNo')}</p>
          <p className="doc-policy">{t('policyNotice')}</p>
          <label className="doc-label" htmlFor={`${fieldId}-instruction`}>
            {t('instruction')}
          </label>
          <textarea
            id={`${fieldId}-instruction`}
            rows={2}
            value={instruction}
            maxLength={8192}
            onChange={(event) => setInstruction(event.target.value)}
          />
          {!textValid(instruction, 8192) ? <p className="doc-warning">{t('invalid')}</p> : null}
          <ExternalResearchInput
            language={language}
            available={researchAvailable}
            busy={busy || Boolean(action)}
            enabled={researchEnabled}
            query={researchQuery}
            onEnabled={setResearchEnabled}
            onQuery={setResearchQuery}
          />
        </details>
        <section
          className="doc-outline"
          hidden={pane !== 'outline'}
          role="tabpanel"
          id={`${fieldId}-outline-pane`}
          aria-labelledby={`${fieldId}-outline-tab`}
        >
          <header>
            <h3>{t('outline')}</h3>
          </header>
          <label className="doc-label" htmlFor={`${fieldId}-plan-title`}>
            {t('planTitle')}
          </label>
          <input
            id={`${fieldId}-plan-title`}
            value={plan.title}
            required
            aria-invalid={!titleValid(plan.title)}
            disabled={!editable}
            maxLength={400}
            onChange={(event) => updatePlan({ ...plan, title: event.target.value })}
          />
          <AssemblyPlanControls
            key={`${id}-${page?.revision.id}`}
            plan={plan}
            language={language}
            disabled={!editable}
            onChange={updatePlan}
          />
          <ol>
            {plan.sections.map((section, index) => (
              <li key={section.id}>
                <label htmlFor={`${fieldId}-${section.id}-heading`}>{t('heading')}</label>
                <input
                  id={`${fieldId}-${section.id}-heading`}
                  value={section.heading}
                  required
                  aria-invalid={!section.heading.trim() || !textValid(section.heading, 1200)}
                  disabled={!editable}
                  maxLength={1200}
                  onChange={(event) =>
                    updatePlan({
                      ...plan,
                      sections: plan.sections.map((item) =>
                        item.id === section.id ? { ...item, heading: event.target.value } : item,
                      ),
                    })
                  }
                />
                <label htmlFor={`${fieldId}-${section.id}-instruction`}>
                  {t('sectionInstruction')}
                </label>
                <textarea
                  id={`${fieldId}-${section.id}-instruction`}
                  value={section.instruction}
                  aria-invalid={!textValid(section.instruction, 8000)}
                  disabled={!editable}
                  rows={2}
                  maxLength={8000}
                  onChange={(event) =>
                    updatePlan({
                      ...plan,
                      sections: plan.sections.map((item) =>
                        item.id === section.id
                          ? { ...item, instruction: event.target.value }
                          : item,
                      ),
                    })
                  }
                />
                <AssemblySectionEditor
                  plan={plan}
                  sectionId={section.id}
                  language={language}
                  disabled={!editable}
                  onChange={updatePlan}
                />
                <button
                  type="button"
                  className="text-button"
                  disabled={!editable}
                  onClick={() =>
                    updatePlan({ ...plan, sections: plan.sections.filter((_, i) => i !== index) })
                  }
                >
                  {t('removeSection')}
                </button>
              </li>
            ))}
          </ol>
          <button
            type="button"
            className="text-button"
            disabled={!editable || plan.sections.length >= 64}
            onClick={() =>
              updatePlan({
                ...plan,
                sections: [
                  ...plan.sections,
                  { id: crypto.randomUUID(), heading: '', instruction: '' },
                ],
              })
            }
          >
            <Plus size={14} />
            {t('addSection')}
          </button>
          {!planValid(plan) && plan.sections.length ? (
            <p className="doc-warning">{t('invalid')}</p>
          ) : null}
          <p id={`${fieldId}-outline-reset`} className="doc-note">
            {t('outlineReset')}
          </p>
          <label className="doc-check">
            <input
              type="checkbox"
              checked={reviewed}
              aria-describedby={`${fieldId}-outline-reset`}
              disabled={!editable}
              onChange={(event) => setReviewed(event.target.checked)}
            />
            <span>{t('reviewed')}</span>
          </label>
        </section>
        <section
          className="doc-blocks"
          hidden={pane !== 'body'}
          role="tabpanel"
          id={`${fieldId}-body-pane`}
          aria-labelledby={`${fieldId}-body-tab`}
        >
          <header>
            <h3>{t('body')}</h3>
          </header>
          <p className="doc-note">{t('reviewRequired')}</p>
          <p className="doc-classification">{t('classification')}</p>
          {blocks.map((block) => (
            <article className="doc-block" key={block.id} data-kind={block.kind}>
              <header>
                <label className="doc-check">
                  <input
                    type="checkbox"
                    aria-label={t('selectedBlocks')}
                    checked={selectedBlocks.includes(block.id)}
                    onChange={(event) =>
                      setSelectedBlocks((previous) =>
                        event.target.checked
                          ? [...previous, block.id]
                          : previous.filter((id) => id !== block.id),
                      )
                    }
                  />
                  {/* The kind is shown by the picker next to it; this text names the checkbox. */}
                  <span className="doc-sr-only">{t(block.kind)}</span>
                </label>
                <label className="doc-sr-only" htmlFor={`${fieldId}-${block.id}-kind`}>
                  {t('kind')}
                </label>
                <select
                  id={`${fieldId}-${block.id}-kind`}
                  value={block.kind}
                  disabled={!editable}
                  onChange={(event) =>
                    updateBlock(block.id, { kind: event.target.value as DocumentBlockKind })
                  }
                >
                  {(['fact', 'userInput', 'aiProposal', 'blank'] as const).map((kind) => (
                    <option
                      key={kind}
                      value={kind}
                      disabled={kind === 'blank' && Boolean(block.markdown)}
                    >
                      {t(kind)}
                    </option>
                  ))}
                </select>

                {block.userEdited ? (
                  <span className="doc-user-edited">{t('userEdited')}</span>
                ) : null}
                <small>
                  {block.sectionId
                    ? plan.sections.find((section) => section.id === block.sectionId)?.heading
                    : null}
                </small>
              </header>
              {block.kind === 'blank' ? <p className="doc-note">{t('blankHelp')}</p> : null}
              <label className="doc-sr-only" htmlFor={`${fieldId}-${block.id}-body`}>
                {t('blockText')}
              </label>
              {/* Prose wraps like a document; the outline and export keep exact Markdown. */}
              <textarea
                id={`${fieldId}-${block.id}-body`}
                className="doc-markdown-input doc-block-text"
                value={block.markdown}
                readOnly={!editable || block.kind === 'blank'}
                maxLength={131072}
                rows={Math.min(18, Math.max(4, block.markdown.split('\n').length))}
                spellCheck={false}
                onChange={(event) => updateBlock(block.id, { markdown: event.target.value })}
                onSelect={(event) => {
                  const input = event.currentTarget;
                  const range = scalarSelection(
                    input.value,
                    input.selectionStart,
                    input.selectionEnd,
                  );
                  setSelection(range ? { blockId: block.id, ...range } : null);
                }}
              />
              {unknownLabels(block).map((label) => (
                <p className="doc-note" key={label}>
                  {documentText(language, 'unknownCitation', { value: label })}
                </p>
              ))}
              <div className="doc-actions">
                <button
                  type="button"
                  className="text-button"
                  disabled={!canAI}
                  onClick={() => void generate('rewrite', [block.id])}
                >
                  {t('rewrite')}
                </button>
                <button
                  type="button"
                  className="text-button"
                  disabled={!editable}
                  onClick={() => {
                    editVersion.current++;
                    setBlocks((previous) => previous.filter((item) => item.id !== block.id));
                    setSelection(null);
                    setSelectedBlocks((previous) => previous.filter((id) => id !== block.id));
                  }}
                >
                  {t('removeBlock')}
                </button>
              </div>
            </article>
          ))}
          {!validBlocks ? (
            <p role="status" className="doc-warning">
              {t('invalid')}
            </p>
          ) : null}
          <div className="doc-actions">
            <button
              type="button"
              className="text-button"
              disabled={!editable || blocks.length >= 256}
              onClick={() => {
                editVersion.current++;
                setBlocks((previous) => [
                  ...previous,
                  {
                    id: crypto.randomUUID(),
                    sectionId: plan.sections[0]?.id ?? null,
                    kind: 'userInput',
                    markdown: '',
                    sourceLabels: [],
                    claimsVerified: false,
                    userEdited: true,
                  },
                ]);
              }}
            >
              <Plus size={14} />
              {t('addBlock')}
            </button>
          </div>
          <p className="doc-note">
            {t('selectionHelp')}
            {selection
              ? ' ' +
                documentText(language, 'selectedRange', {
                  start: selection.start,
                  end: selection.end,
                })
              : ''}
          </p>
        </section>
        <SourceRecords
          key={page.revision.id}
          revision={page.revision}
          policyKey={policyKey}
          language={language}
        />
        <details className="doc-markdown">
          <summary>{t('markdown')}</summary>
          <pre>{page.revision.markdown}</pre>
        </details>
        {page.revision.usage !== null ? (
          <details className="doc-usage">
            <summary>{t('usage')}</summary>
            <pre>{JSON.stringify(page.revision.usage, null, 2)}</pre>
          </details>
        ) : null}
        <details className="doc-export">
          <summary>{t('exportFiles')}</summary>
          <label className="doc-check">
            <input
              type="checkbox"
              checked={exportSources}
              disabled={Boolean(action)}
              onChange={(event) => setExportSources(event.target.checked)}
            />
            <span>{t('exportSources')}</span>
          </label>
          <label className="doc-check">
            <input
              type="checkbox"
              checked={exportMarkers}
              disabled={Boolean(action)}
              onChange={(event) => setExportMarkers(event.target.checked)}
            />
            <span>{t('exportMarkers')}</span>
          </label>
          <button
            type="button"
            className="button"
            disabled={Boolean(action) || dirty}
            onClick={() => void runAction('export')}
          >
            <Download size={15} />
            {t('export')} · {documentText(language, 'version', { value: page.revision.sequence })}
          </button>
          <button
            type="button"
            className="button"
            disabled={Boolean(action) || dirty}
            onClick={() => void runAction('exportPdf')}
          >
            <Download size={15} />
            {t(action === 'exportPdf' ? 'pdfExporting' : 'exportPdf')} ·{' '}
            {documentText(language, 'version', { value: exportVersion ?? page.revision.sequence })}
          </button>
          <button
            type="button"
            className="button"
            disabled={Boolean(action) || dirty}
            onClick={() => void runAction('exportHwpx')}
          >
            <Download size={15} />
            {t(action === 'exportHwpx' ? 'hwpxExporting' : 'exportHwpx')} ·{' '}
            {documentText(language, 'version', { value: exportVersion ?? page.revision.sequence })}
          </button>
          {dirty ? <p className="muted">{t('exportSaveFirst')}</p> : null}
          <p className="muted">{t('pdfImages')}</p>
          <p className="muted">{t('hwpxHelp')}</p>
          <p className="muted">{t('hancomAttribution')}</p>
          {action === 'exportPdf' || action === 'exportHwpx' ? (
            <button
              type="button"
              className="button subtle"
              disabled={exportStopping || !exportRunning}
              onClick={() => {
                const pending = exportRequest.current;
                if (!pending) return;
                setExportStopping(true);
                void documents[pending.format === 'pdf' ? 'cancelPdf' : 'cancelHwpx'](
                  pending.request,
                )
                  .then((cancelled) => {
                    if (!cancelled && alive.current) setExportStopping(false);
                  })
                  .catch((cause) => {
                    if (alive.current) {
                      setExportStopping(false);
                      setError(failure(cause));
                    }
                  });
              }}
            >
              <Square size={14} />
              {t(action === 'exportPdf' ? 'pdfStop' : 'hwpxStop')}
            </button>
          ) : null}
        </details>
        <details className="doc-history">
          <summary>{t('history')}</summary>
          <header>
            <h3>{t('history')}</h3>
            <button
              type="button"
              className="text-button"
              disabled={historyBusy}
              onClick={() => void fetchHistory()}
            >
              {t('refresh')}
            </button>
          </header>
          <p className="doc-note">{t('historyHelp')}</p>
          <div className="doc-actions">
            <button
              type="button"
              className="button"
              disabled={
                !page.revision.parentRevisionId || dirty || busy || Boolean(action) || historical
              }
              onClick={() => void commit('restore', page.revision.parentRevisionId ?? undefined)}
            >
              {t('undo')}
            </button>
            {historical ? (
              <button
                type="button"
                className="button"
                disabled={dirty || busy || Boolean(action)}
                onClick={() => void commit('restore', page.revision.id)}
              >
                {t('restore')}
              </button>
            ) : null}
          </div>
          <ul>
            {history.map((version) => (
              <li key={version.id}>
                <button
                  type="button"
                  disabled={dirty || loading}
                  aria-current={version.id === page.revision.id ? 'true' : undefined}
                  onClick={() => void openRevision(version.id)}
                >
                  <span>
                    {documentText(language, 'version', { value: version.sequence })} ·{' '}
                    {t(`${version.operation}Operation` as DocumentMessage)}
                  </span>
                  <time dateTime={new Date(version.createdMs).toISOString()}>
                    {new Date(version.createdMs).toLocaleString(language)}
                  </time>
                </button>
              </li>
            ))}
          </ul>
          {historyMore ? (
            <button
              type="button"
              className="button"
              disabled={historyBusy}
              onClick={() => void fetchHistory(true)}
            >
              {t('moreVersions')}
            </button>
          ) : null}
        </details>
      </div>
    </section>
  );
}
export interface DocumentWorkspaceProps {
  onModels: () => void;
  onScopes: () => void;
  onLocalDraft?: () => void;
  onDirtyChange?: (dirty: boolean) => void;
}
export function DocumentWorkspace({
  onModels,
  onScopes,
  onLocalDraft,
  onDirtyChange,
}: DocumentWorkspaceProps) {
  const { settings, ready, session } = useClient();
  const language = settings.language;
  const t = (key: DocumentMessage) => documentText(language, key);
  const id = useId();
  // An unknown or filename-only project has no document retrieval scopes.
  // Do not interpret a missing project descriptor as permission to use all folders.
  const projectId = session.scopeId;
  const project = settings.scopes.find((scope) => scope.id === projectId);
  const scopes = settings.scopes.filter(
    (scope) => scope.contentEnabled && (!projectId || scope.id === projectId),
  );
  const scopesKey = JSON.stringify(
    scopes.map((scope) => [scope.id, scope.path, scope.externalAllowed]),
  );
  const policyKey = JSON.stringify([
    projectId,
    settings.revision,
    scopesKey,
    settings.generation,
    settings.mcp,
  ]);
  const [scopeChoice, setScopeChoice] = useState<{
    projectId: string | null;
    ids: string[] | null;
  }>({ projectId, ids: null });
  const setScopeIds = (ids: string[]) => setScopeChoice({ projectId, ids });
  const [query, setQuery] = useState('');
  const [list, setList] = useState<{ query: string; items: DocumentSummary[] } | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<DocumentFailure | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  // "New document" or "fill an existing form"; starts from the user's setting (Q46).
  const [mode, setMode] = useState(settings.documents.defaultMode);
  const [editorDirty, setEditorDirty] = useState(false);
  const [formDirty, setFormDirty] = useState(false);
  const [creating, setCreating] = useState(false);
  const [title, setTitle] = useState('');
  const [purpose, setPurpose] = useState('');
  const [template, setTemplate] = useState('');
  const navigationBlocked =
    editorDirty || formDirty || creating || (showCreate && Boolean(title || purpose || template));
  useEffect(() => {
    onDirtyChange?.(navigationBlocked);
    return () => onDirtyChange?.(false);
  }, [navigationBlocked, onDirtyChange]);
  const alive = useRef(false);
  const createPending = useRef(false);
  const createVersion = useRef(0);
  const selectionEpoch = useRef(0);
  const listEpoch = useRef(0);
  const deleted = useRef(new Set<string>());
  // Derive the new project's defaults during render, before effects run. A late
  // saved-revision read from another project cannot reintroduce its selection.
  const scopeIds = scopeChoice.projectId === projectId ? scopeChoice.ids : null;
  const selectedScopes = (scopeIds ?? (ready ? scopes.map((scope) => scope.id) : [])).filter(
    (scopeId) => scopes.some((scope) => scope.id === scopeId),
  );
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      selectionEpoch.current++;
      listEpoch.current++;
    };
  }, []);
  useEffect(() => {
    if (ready)
      setScopeChoice((previous) => ({
        projectId,
        ids:
          previous.projectId !== projectId || previous.ids === null
            ? scopes.map((scope) => scope.id)
            : previous.ids.filter((id) => scopes.some((scope) => scope.id === id)),
      }));
  }, [ready, scopesKey, projectId]);
  useEffect(() => {
    if (!native) return;
    const token = ++listEpoch.current;
    let disposed = false;
    setLoading(true);
    setError(null);
    const timer = setTimeout(
      () => {
        void documents
          .list(query, 100)
          .then((items) => {
            if (!disposed && alive.current && token === listEpoch.current)
              setList({ query, items });
          })
          .catch((cause) => {
            if (!disposed && alive.current && token === listEpoch.current) setError(failure(cause));
          })
          .finally(() => {
            if (!disposed && alive.current && token === listEpoch.current) setLoading(false);
          });
      },
      query ? 200 : 0,
    );
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [query, refresh]);
  const canCreate =
    titleValid(title) &&
    Boolean(purpose.trim()) &&
    textValid(purpose, 8192) &&
    textValid(template, 65536) &&
    selectedScopes.length <= 128;
  const create = async () => {
    if (!native || !canCreate || createPending.current) return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    const selection = selectionEpoch.current;
    const version = createVersion.current;
    try {
      const page = await documents.create({
        title,
        purpose,
        scopeIds: [...selectedScopes],
        templateMarkdown: template || null,
      });
      if (alive.current) {
        if (selectionEpoch.current === selection && createVersion.current === version) {
          selectionEpoch.current++;
          setSelected(page.document.id);
          setShowCreate(false);
          setTitle('');
          setPurpose('');
          setTemplate('');
        }
        setRefresh((value) => value + 1);
      }
    } catch (cause) {
      if (alive.current) setError(failure(cause));
    } finally {
      createPending.current = false;
      if (alive.current) setCreating(false);
    }
  };
  const items =
    list?.query === query ? list.items.filter((item) => !deleted.current.has(item.id)) : [];
  return (
    <section className="feature-view document-workspace" aria-labelledby={`${id}-title`}>
      <header className="doc-workspace-heading">
        <div>
          <h2 id={`${id}-title`}>{t('title')}</h2>
          <p>{t('subtitle')}</p>
        </div>
        <div className="doc-actions">
          {/* Settings links stay light so the document itself is the focus. */}
          <button type="button" className="text-button" onClick={onScopes}>
            <FolderSearch size={14} />
            {t('scopes')}
          </button>
          <button type="button" className="text-button" onClick={onModels}>
            <Settings2 size={14} />
            {t('models')}
          </button>
          {onLocalDraft ? (
            <button
              type="button"
              className="text-button"
              disabled={navigationBlocked}
              title={navigationBlocked ? t('unsaved') : undefined}
              onClick={() => {
                if (!navigationBlocked) onLocalDraft();
              }}
            >
              {t('localDraft')}
            </button>
          ) : null}
        </div>
      </header>
      {projectId ? (
        <p className="doc-project" role="status">
          <FolderSearch size={14} aria-hidden="true" />
          <span>
            {project
              ? documentText(
                  language,
                  project.contentEnabled ? 'projectScope' : 'projectNoContent',
                  {
                    name: project.name,
                  },
                )
              : t('projectMissing')}
          </span>
        </p>
      ) : null}
      <div className="doc-mode" role="group" aria-label={t('modeLabel')}>
        {(['new', 'fill'] as const).map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={mode === value}
            className={mode === value ? 'active' : undefined}
            disabled={mode !== value && navigationBlocked}
            onClick={() => {
              if (!navigationBlocked || mode === value) setMode(value);
            }}
          >
            {t(value === 'new' ? 'modeNew' : 'modeFill')}
          </button>
        ))}
      </div>
      <details className="doc-support">
        <summary>{t('supportedFormats')}</summary>
        <p className="doc-note">{t('supported')}</p>
      </details>
      {!native ? (
        <div className="doc-native" role="status">
          <FileText size={48} strokeWidth={1.25} />
          <p>{t('nativeOnly')}</p>
        </div>
      ) : mode === 'fill' ? (
        <FormFillPanel language={language} onDirtyChange={setFormDirty} />
      ) : (
        <>
          <div className="doc-layout">
            <aside className="doc-sidebar" aria-labelledby={`${id}-list`}>
              <header>
                <h3 id={`${id}-list`}>{t('documents')}</h3>
                <button
                  type="button"
                  className="icon-button small"
                  aria-label={t('refresh')}
                  disabled={loading}
                  onClick={() => setRefresh((value) => value + 1)}
                >
                  <RefreshCw size={15} />
                </button>
              </header>
              {/* Screen readers keep the label; on screen the placeholder says it. */}
              <label className="doc-sr-only" htmlFor={`${id}-search`}>
                {t('search')}
              </label>
              <input
                id={`${id}-search`}
                type="search"
                value={query}
                maxLength={256}
                placeholder={t('search')}
                onChange={(event) => setQuery(event.target.value)}
              />
              <button
                type="button"
                className="button"
                disabled={navigationBlocked}
                title={navigationBlocked ? t('unsaved') : undefined}
                onClick={() => {
                  selectionEpoch.current++;
                  setShowCreate(true);
                }}
              >
                <Plus size={15} />
                {t('newDocument')}
              </button>
              {loading ? (
                <p className="doc-note" role="status">
                  {t('loading')}
                </p>
              ) : null}
              {!loading && !error && !items.length ? (
                <p className="doc-note">{t(query ? 'noMatches' : 'empty')}</p>
              ) : null}
              <ul>
                {items.map((item) => (
                  <li key={item.id}>
                    <button
                      type="button"
                      disabled={navigationBlocked}
                      title={navigationBlocked ? t('unsaved') : undefined}
                      aria-current={selected === item.id && !showCreate ? 'true' : undefined}
                      onClick={() => {
                        selectionEpoch.current++;
                        setSelected(item.id);
                        setShowCreate(false);
                      }}
                    >
                      <strong>{item.title}</strong>
                      {/* Version count and last change, as one quiet line. */}
                      <small>
                        {documentText(language, 'version', { value: item.revisionCount })}
                        <time dateTime={new Date(item.updatedMs).toISOString()}>
                          {new Date(item.updatedMs).toLocaleDateString(language)}
                        </time>
                      </small>
                    </button>
                  </li>
                ))}
              </ul>
            </aside>
            <div className="doc-main">
              {error ? <Failure error={error} language={language} onModels={onModels} /> : null}
              {showCreate ? (
                <section className="doc-create">
                  <h3>{t('newDocument')}</h3>
                  <label className="doc-label" htmlFor={`${id}-new-title`}>
                    {t('docTitle')}
                  </label>
                  <input
                    id={`${id}-new-title`}
                    value={title}
                    maxLength={400}
                    onChange={(event) => {
                      createVersion.current++;
                      setTitle(event.target.value);
                    }}
                  />
                  <label className="doc-label" htmlFor={`${id}-purpose`}>
                    {t('purpose')}
                  </label>
                  <textarea
                    id={`${id}-purpose`}
                    value={purpose}
                    maxLength={8192}
                    rows={3}
                    onChange={(event) => {
                      createVersion.current++;
                      setPurpose(event.target.value);
                    }}
                  />
                  <p className="doc-note">{t('purposeHelp')}</p>
                  <details className="doc-template">
                    <summary>{t('template')}</summary>
                    <label className="doc-label" htmlFor={`${id}-template`}>
                      {t('template')}
                    </label>
                    <textarea
                      id={`${id}-template`}
                      className="doc-markdown-input"
                      value={template}
                      maxLength={65536}
                      rows={5}
                      spellCheck={false}
                      onChange={(event) => {
                        createVersion.current++;
                        setTemplate(event.target.value);
                      }}
                    />
                    <p className="doc-note">{t('templateHelp')}</p>
                  </details>
                  <ScopePicker
                    scopes={scopes}
                    selected={selectedScopes}
                    onChange={setScopeIds}
                    language={language}
                    onScopes={onScopes}
                  />
                  <p className="doc-note">{t('createHelp')}</p>
                  {!canCreate && (title || purpose || template) ? (
                    <p className="doc-note">{t('invalid')}</p>
                  ) : null}
                  <div className="doc-actions">
                    <button
                      type="button"
                      className="button primary"
                      disabled={!canCreate || creating}
                      onClick={() => void create()}
                    >
                      {t(creating ? 'working' : 'create')}
                    </button>
                    <button
                      type="button"
                      className="text-button"
                      disabled={creating}
                      onClick={() => setShowCreate(false)}
                    >
                      {t('cancel')}
                    </button>
                  </div>
                </section>
              ) : selected ? (
                <DocumentEditor
                  key={selected}
                  id={selected}
                  language={language}
                  scopes={scopes}
                  scopeIds={selectedScopes}
                  onScopeIds={setScopeIds}
                  policyKey={policyKey}
                  externalQuestions={settings.generation.allowExternalQuestions}
                  researchAvailable={
                    settings.mcp.enabled &&
                    settings.mcp.servers.some((server) => Boolean(server.search))
                  }
                  onModels={onModels}
                  onScopes={onScopes}
                  onChanged={() => setRefresh((value) => value + 1)}
                  onDirty={setEditorDirty}
                  onDeleted={() => {
                    deleted.current.add(selected);
                    selectionEpoch.current++;
                    listEpoch.current++;
                    setSelected(null);
                    setRefresh((value) => value + 1);
                  }}
                />
              ) : (
                <div className="doc-empty">
                  <FileText size={40} strokeWidth={1.25} />
                  <p>{t('select')}</p>
                </div>
              )}
            </div>
          </div>
        </>
      )}
    </section>
  );
}
