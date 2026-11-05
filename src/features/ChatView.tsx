import { useEffect, useId, useRef, useState } from 'react';
import {
  Download,
  FolderSearch,
  MessageSquareText,
  Plus,
  RefreshCw,
  Send,
  Square,
  X,
} from 'lucide-react';
import { bridge, native } from '../lib/bridge';
import { content } from '../lib/content';
import type { ContentChunk } from '../lib/content';
import { rag } from '../lib/rag';
import type {
  Citation,
  ConversationPage,
  ConversationSummary,
  ConversationTurn,
  RagJob,
  SourceRef,
} from '../lib/rag';
import { useClient } from '../lib/store';
import type { Language, SearchScope } from '../lib/types';
import { chatText } from './chat/messages';
import type { ChatMessage } from './chat/messages';
import './chat/chat.css';
import { SearchSelection } from './chat/SearchSelection';

const errorCodes = new Set([
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
function failureCode(cause: unknown) {
  const code = cause && typeof cause === 'object' && 'code' in cause ? cause.code : null;
  return typeof code === 'string' && errorCodes.has(code) ? code : 'worker';
}
const configErrors = new Set(['notConfigured', 'auth', 'endpoint', 'unsupported']);
const policyErrors = new Set(['scopeBlocked', 'policyChanged', 'sourceChanged']);
function Failure({
  code,
  language,
  onModels,
}: {
  code: string;
  language: Language;
  onModels: () => void;
}) {
  const kind = configErrors.has(code)
    ? 'settingsError'
    : policyErrors.has(code)
      ? 'policyError'
      : code === 'noSources'
        ? 'noSources'
        : code === 'cancelled'
          ? 'cancelled'
          : 'error';
  return (
    <div className="chat-failure" role="alert">
      <p>
        {chatText(language, kind)} <code>{code}</code>
      </p>
      {configErrors.has(code) ? (
        <button type="button" className="text-button" onClick={onModels}>
          {chatText(language, 'models')}
        </button>
      ) : null}
    </div>
  );
}
function basename(path: string) {
  return path.split(/[\\/]/u).pop() || path;
}
function mergeTurns(previous: ConversationTurn[], incoming: ConversationTurn[]) {
  const turns = new Map(previous.map((turn) => [turn.id, turn]));
  for (const turn of incoming) turns.set(turn.id, turn);
  return [...turns.values()].sort((a, b) => a.sequence - b.sequence);
}
function sameSource(a: SourceRef, b: SourceRef) {
  return (
    a.chunkId === b.chunkId &&
    a.scopeId === b.scopeId &&
    a.filePath === b.filePath &&
    a.sha256 === b.sha256 &&
    a.page === b.page &&
    a.paragraph === b.paragraph
  );
}
function chunkMatches(chunk: ContentChunk, source: SourceRef) {
  return (
    chunk.id === source.chunkId &&
    chunk.filePath === source.filePath &&
    chunk.fingerprint.sha256 === source.sha256 &&
    chunk.page === source.page &&
    chunk.paragraph === source.paragraph
  );
}
function activeJob(job: RagJob | null) {
  return job?.phase === 'retrieving' || job?.phase === 'generating';
}

function Turn({
  turn,
  language,
  onModels,
  onPreview,
  onToggle,
  toggling,
}: {
  turn: ConversationTurn;
  language: Language;
  onModels: () => void;
  onPreview: (turn: ConversationTurn, citation: Citation) => void;
  onToggle: (turn: ConversationTurn) => void;
  toggling: boolean;
}) {
  const t = (key: ChatMessage) => chatText(language, key);
  return (
    <li className="chat-turn" data-role={turn.role} data-status={turn.status}>
      <article aria-labelledby={`turn-${turn.id}`}>
        <header>
          <strong id={`turn-${turn.id}`}>{t(turn.role === 'user' ? 'user' : 'assistant')}</strong>
          <time dateTime={new Date(turn.createdMs).toISOString()}>
            {new Date(turn.createdMs).toLocaleString(language)}
          </time>
          <span className="chat-turn-status">{t(turn.status)}</span>
        </header>
        {turn.text ? <p className="chat-plain">{turn.text}</p> : null}
        {turn.role === 'assistant' ? (
          <>
            {turn.status === 'noSources' ? <p className="chat-note">{t('noSources')}</p> : null}
            {turn.status === 'interrupted' ? <p className="chat-note">{t('interrupted')}</p> : null}
            {turn.status === 'cancelled' ? <p className="chat-note">{t('cancelled')}</p> : null}
            {turn.evidence === 'sourceLinked' ? (
              <p className="chat-evidence">{t('sourceLinked')}</p>
            ) : turn.status === 'complete' ? (
              <p className="chat-evidence">
                {t(turn.citations.length ? 'incompleteEvidence' : 'unverified')}
              </p>
            ) : null}
            {turn.error ? (
              <Failure code={failureCode(turn.error)} language={language} onModels={onModels} />
            ) : null}
            {turn.provider || turn.reportedModel || turn.requestedModel ? (
              <p className="chat-note">
                {t('provider')}: {turn.provider ?? '—'} ·{' '}
                {turn.reportedModel ?? turn.requestedModel ?? '—'}
                {turn.finishReason ? ` · ${turn.finishReason}` : ''}
              </p>
            ) : null}
            {turn.citations.length ? (
              <div className="chat-citations">
                <button
                  type="button"
                  className="text-button"
                  disabled={toggling}
                  onClick={() => onToggle(turn)}
                >
                  {t(turn.sourcesHidden ? 'showTurnSources' : 'hideTurnSources')}
                </button>
                {turn.sourcesHidden ? (
                  <p className="chat-note">{t('hiddenHelp')}</p>
                ) : (
                  <>
                    <h4>{t('citations')}</h4>
                    <ul>
                      {turn.citations.map((citation) => (
                        <li key={`${citation.label}:${citation.source.chunkId}`}>
                          <div>
                            <strong>[{citation.label}]</strong>{' '}
                            <span className="chat-source-state" data-state={citation.state}>
                              {t(citation.state)}
                            </span>
                            <small>{t(citation.isCited ? 'cited' : 'retrieved')}</small>
                          </div>
                          <span className="chat-source-name">
                            {basename(citation.source.filePath)}
                          </span>
                          <details className="chat-source-details">
                            <summary>{t('sourceDetails')}</summary>
                            <code className="chat-source-path">{citation.source.filePath}</code>
                          </details>
                          <span className="chat-source-location">
                            {citation.source.page !== null
                              ? chatText(language, 'page', { value: citation.source.page })
                              : null}
                            {citation.source.page !== null && citation.source.paragraph !== null
                              ? ' · '
                              : null}
                            {citation.source.paragraph !== null
                              ? chatText(language, 'paragraph', {
                                  value: citation.source.paragraph,
                                })
                              : null}
                          </span>
                          {citation.state === 'current' ? (
                            <button
                              type="button"
                              className="text-button"
                              onClick={() => onPreview(turn, citation)}
                            >
                              {t('preview')}
                            </button>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  </>
                )}
              </div>
            ) : null}
            {turn.unknownCitations.length ? (
              <p className="chat-note">
                {chatText(language, 'unknownCitations', {
                  labels: turn.unknownCitations.join(', '),
                })}
              </p>
            ) : null}
            {turn.usage !== null ? (
              <details className="chat-usage">
                <summary>{t('usage')}</summary>
                <pre>{JSON.stringify(turn.usage, null, 2)}</pre>
              </details>
            ) : null}
          </>
        ) : null}
      </article>
    </li>
  );
}
interface Preview {
  turnId: string;
  sequence: number;
  source: SourceRef;
  chunk: ContentChunk;
  policyKey: string;
}
function ConversationWorkspace({
  id,
  language,
  scopes,
  selectedScopeIds,
  onScopes,
  policyKey,
  allowExternalQuestions,
  onSettings,
  onModels,
  onChanged,
  onDeleted,
}: {
  id: string;
  language: Language;
  scopes: SearchScope[];
  selectedScopeIds: string[];
  onScopes: (ids: string[]) => void;
  policyKey: string;
  allowExternalQuestions: boolean;
  onSettings: () => void;
  onModels: () => void;
  onChanged: () => void;
  onDeleted: () => void;
}) {
  const fieldId = useId();
  const previewDialog = useRef<HTMLDialogElement>(null);
  const t = (key: ChatMessage) => chatText(language, key);
  const alive = useRef(false);
  const loadEpoch = useRef(0);
  const previewEpoch = useRef(0);
  const asking = useRef(false);
  const questionVersion = useRef(0);
  const localRequest = useRef<string | null>(null);
  const runningAction = useRef(false);
  const olderPending = useRef(false);
  const hiddenPending = useRef(new Set<string>());
  const currentPolicy = useRef(policyKey);
  currentPolicy.current = policyKey;
  const [page, setPage] = useState<{ key: string; value: ConversationPage } | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [question, setQuestion] = useState('');
  const [selectionDirty, setSelectionDirty] = useState(false);
  const [mode, setMode] = useState<'documents' | 'general'>('documents');
  const [sourcesHidden, setSourcesHidden] = useState(false);
  const [job, setJob] = useState<RagJob | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [action, setAction] = useState<'rename' | 'delete' | 'export' | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [includeSources, setIncludeSources] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [toggling, setToggling] = useState<string[]>([]);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState(false);
  const [opening, setOpening] = useState(false);
  const view = page?.key === policyKey ? page.value : null;
  const pendingTurn =
    view?.turns.some((turn) => turn.role === 'assistant' && turn.status === 'pending') ?? false;
  const busy = submitting || activeJob(job) || pendingTurn;
  const usableScopeIds = selectedScopeIds.filter((scopeId) =>
    scopes.some((scope) => scope.id === scopeId),
  );
  const questionBytes = new TextEncoder().encode(question.trim()).length;
  const titleValid = Boolean(
    newTitle.trim() &&
    Array.from(newTitle).length <= 200 &&
    !/[\u0000-\u001f\u007f]/u.test(newTitle),
  );
  const canSend = Boolean(
    view &&
    !loading &&
    !loadError &&
    questionBytes <= 4096 &&
    usableScopeIds.length <= 128 &&
    !busy &&
    !action &&
    !selectionDirty &&
    question.trim() &&
    (mode === 'general' ||
      (usableScopeIds.length && (!view?.selection || view.selection.selectedDocuments.length))),
  );
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      loadEpoch.current++;
      previewEpoch.current++;
    };
  }, []);
  useEffect(() => {
    const epoch = ++loadEpoch.current;
    let disposed = false;
    setLoading(true);
    setLoadError(null);
    void rag
      .conversation(id)
      .then((next) => {
        if (!disposed && alive.current && epoch === loadEpoch.current) {
          setPage({ key: policyKey, value: next });
          setNewTitle(next.conversation.title);
        }
      })
      .catch((cause) => {
        if (!disposed && alive.current && epoch === loadEpoch.current)
          setLoadError(failureCode(cause));
      })
      .finally(() => {
        if (!disposed && alive.current && epoch === loadEpoch.current) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [id, policyKey, revision]);
  useEffect(() => {
    setPreview(null);
    setPreviewError(false);
    setPreviewLoading(false);
    previewEpoch.current++;
  }, [id, policyKey]);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let lastPhase: string | null = null;
    const poll = async () => {
      try {
        const next = await rag.job(id);
        if (disposed || !alive.current) return;
        setJob(next);
        const phase = next ? `${next.requestId}:${next.phase}` : 'none';
        if (lastPhase !== null && phase !== lastPhase) setRevision((value) => value + 1);
        lastPhase = phase;
      } catch (cause) {
        if (!disposed && alive.current) setAskError(failureCode(cause));
      }
      if (!disposed) timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [id]);
  const loadOlder = async () => {
    if (!view?.hasMore || olderPending.current || loading) return;
    olderPending.current = true;
    setLoadingOlder(true);
    setActionError(null);
    const epoch = loadEpoch.current;
    const key = policyKey;
    const before = view.turns[0]?.sequence;
    try {
      if (before === undefined) return;
      const next = await rag.conversation(id, before, 100);
      if (alive.current && epoch === loadEpoch.current && currentPolicy.current === key)
        setPage((previous) =>
          previous?.key === key
            ? {
                key,
                value: {
                  ...previous.value,
                  turns: mergeTurns(next.turns, previous.value.turns),
                  hasMore: next.hasMore,
                },
              }
            : previous,
        );
    } catch (cause) {
      if (alive.current && epoch === loadEpoch.current) setActionError(failureCode(cause));
    } finally {
      olderPending.current = false;
      if (alive.current) setLoadingOlder(false);
    }
  };
  const submit = async () => {
    if (!canSend || asking.current) return;
    const selected = [...usableScopeIds];
    if (mode === 'documents' && !selected.length) return;
    const sent = question.trim();
    const questionAtSubmit = questionVersion.current;
    const requestId = crypto.randomUUID();
    asking.current = true;
    localRequest.current = requestId;
    setSubmitting(true);
    setAskError(null);
    setCancelRequested(false);
    try {
      await rag.ask({
        requestId,
        conversationId: id,
        question: sent,
        scopeIds: selected,
        mode,
        sourcesHidden,
      });
      if (alive.current) {
        if (questionVersion.current === questionAtSubmit) setQuestion('');
        onChanged();
      }
    } catch (cause) {
      if (alive.current) setAskError(failureCode(cause));
    } finally {
      asking.current = false;
      localRequest.current = null;
      if (alive.current) {
        setSubmitting(false);
        setRevision((value) => value + 1);
      }
    }
  };
  const cancel = async () => {
    const requestId =
      localRequest.current ??
      (activeJob(job) ? job?.requestId : null) ??
      view?.turns.find((turn) => turn.role === 'assistant' && turn.status === 'pending')?.requestId;
    if (!requestId || cancelRequested) return;
    setCancelRequested(true);
    try {
      await rag.cancel(id, requestId);
    } catch (cause) {
      if (alive.current) {
        setAskError(failureCode(cause));
        setCancelRequested(false);
      }
    }
  };
  const mutate = async (kind: 'rename' | 'delete' | 'export') => {
    if (runningAction.current || !view || (kind !== 'export' && busy)) return;
    runningAction.current = true;
    setAction(kind);
    setActionError(null);
    setNotice(null);
    try {
      if (kind === 'rename') {
        if (!titleValid) return;
        await rag.rename(id, newTitle.trim());
        if (alive.current) {
          setRenaming(false);
          setRevision((value) => value + 1);
          onChanged();
        }
      } else if (kind === 'delete') {
        await rag.remove(id);
        if (alive.current) onDeleted();
      } else {
        const markdown = await rag.export(id, includeSources);
        if (!alive.current) return;
        const saved = await bridge.exportMarkdown(view.conversation.title, markdown);
        if (saved && alive.current) setNotice('exported');
      }
    } catch (cause) {
      if (alive.current) setActionError(failureCode(cause));
    } finally {
      runningAction.current = false;
      if (alive.current) setAction(null);
    }
  };
  const toggleHidden = async (turn: ConversationTurn) => {
    if (hiddenPending.current.has(turn.id)) return;
    hiddenPending.current.add(turn.id);
    setToggling([...hiddenPending.current]);
    const key = policyKey;
    setActionError(null);
    setPreview(null);
    previewEpoch.current++;
    try {
      await rag.sourcesHidden(turn.id, !turn.sourcesHidden);
      if (alive.current && currentPolicy.current === key)
        setPage((previous) =>
          previous?.key === key
            ? {
                key,
                value: {
                  ...previous.value,
                  turns: previous.value.turns.map((item) =>
                    item.id === turn.id ? { ...item, sourcesHidden: !turn.sourcesHidden } : item,
                  ),
                },
              }
            : previous,
        );
    } catch (cause) {
      if (alive.current && currentPolicy.current === key) setActionError(failureCode(cause));
    } finally {
      hiddenPending.current.delete(turn.id);
      if (alive.current) setToggling([...hiddenPending.current]);
    }
  };
  const freshChunk = async (
    turnId: string,
    sequence: number,
    source: SourceRef,
    previewRequest: number,
  ) => {
    const fresh = await rag.conversation(id, sequence + 1, 1);
    const turn = fresh.turns.find((item) => item.id === turnId && item.sequence === sequence);
    if (
      turn &&
      alive.current &&
      previewRequest === previewEpoch.current &&
      currentPolicy.current === policyKey
    )
      setPage((previous) =>
        previous?.key === policyKey
          ? {
              ...previous,
              value: {
                ...previous.value,
                turns: previous.value.turns.map((item) => (item.id === turnId ? turn : item)),
              },
            }
          : previous,
      );
    const citation = turn?.citations.find(
      (item) => item.state === 'current' && sameSource(item.source, source),
    );
    if (!turn || turn.sourcesHidden || !citation) throw new Error('sourceUnavailable');
    if (
      !alive.current ||
      previewRequest !== previewEpoch.current ||
      currentPolicy.current !== policyKey
    )
      throw new Error('sourceUnavailable');
    const chunks = await content.chunks([source.chunkId]);
    const chunk = chunks.find((item) => chunkMatches(item, source));
    if (!chunk) throw new Error('sourceUnavailable');
    return chunk;
  };
  const showPreview = async (turn: ConversationTurn, citation: Citation) => {
    const epoch = ++previewEpoch.current;
    const key = policyKey;
    setPreview(null);
    setPreviewError(false);
    setPreviewLoading(true);
    try {
      const chunk = await freshChunk(turn.id, turn.sequence, citation.source, epoch);
      if (alive.current && epoch === previewEpoch.current && currentPolicy.current === key)
        setPreview({
          turnId: turn.id,
          sequence: turn.sequence,
          source: citation.source,
          chunk,
          policyKey: key,
        });
    } catch {
      if (alive.current && epoch === previewEpoch.current && currentPolicy.current === key)
        setPreviewError(true);
    } finally {
      if (alive.current && epoch === previewEpoch.current) setPreviewLoading(false);
    }
  };
  const openPreview = async () => {
    if (!preview || opening || preview.policyKey !== policyKey) return;
    const epoch = previewEpoch.current;
    const key = policyKey;
    setOpening(true);
    try {
      await freshChunk(preview.turnId, preview.sequence, preview.source, epoch);
      if (alive.current && epoch === previewEpoch.current && currentPolicy.current === key)
        await content.open(preview.source.filePath);
    } catch {
      if (alive.current && epoch === previewEpoch.current) {
        setPreview(null);
        setPreviewError(true);
      }
    } finally {
      if (alive.current) setOpening(false);
    }
  };
  useEffect(() => {
    if (!preview || preview.policyKey !== policyKey) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const epoch = previewEpoch.current;
    const validate = async () => {
      try {
        await freshChunk(preview.turnId, preview.sequence, preview.source, epoch);
      } catch {
        if (!disposed && alive.current && epoch === previewEpoch.current) {
          setPreview(null);
          setPreviewError(true);
        }
      }
      if (!disposed) timer = setTimeout(() => void validate(), 2000);
    };
    timer = setTimeout(() => void validate(), 2000);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [preview, policyKey]);
  const closePreview = () => {
    previewEpoch.current++;
    setPreview(null);
    setPreviewError(false);
    setPreviewLoading(false);
  };
  const previewVisible = Boolean(
    previewLoading || previewError || preview?.policyKey === policyKey,
  );
  useEffect(() => {
    if (previewVisible && !previewDialog.current?.open) previewDialog.current?.showModal();
  }, [previewVisible]);
  return (
    <section
      className="chat-workspace"
      aria-label={view ? view.conversation.title || t('searchResultsTitle') : t('loading')}
    >
      <header className="chat-conversation-heading">
        <h2>{view ? view.conversation.title || t('searchResultsTitle') : t('loading')}</h2>
        <div className="chat-actions">
          <button
            type="button"
            className="icon-button small"
            title={t('refresh')}
            aria-label={t('refresh')}
            disabled={loading}
            onClick={() => setRevision((value) => value + 1)}
          >
            <RefreshCw size={15} />
          </button>
          <button
            type="button"
            className="text-button"
            disabled={!view || busy || Boolean(action)}
            onClick={() => {
              setRenaming((value) => !value);
              setConfirmDelete(false);
            }}
          >
            {t('rename')}
          </button>
          <button
            type="button"
            className="text-button chat-danger"
            disabled={!view || busy || Boolean(action)}
            onClick={() => {
              setConfirmDelete(true);
              setRenaming(false);
            }}
          >
            {t('remove')}
          </button>
          <button
            type="button"
            className="text-button"
            disabled={!view || Boolean(action)}
            onClick={() => void mutate('export')}
          >
            <Download size={14} />
            {t('export')}
          </button>
        </div>
      </header>
      <label className="chat-check">
        <input
          type="checkbox"
          checked={includeSources}
          onChange={(event) => setIncludeSources(event.target.checked)}
        />
        <span>{t('exportSources')}</span>
      </label>
      {renaming ? (
        <div className="chat-inline-action">
          <label htmlFor={`${fieldId}-title`}>{t('titleInput')}</label>
          <input
            id={`${fieldId}-title`}
            value={newTitle}
            maxLength={400}
            aria-invalid={!titleValid}
            aria-describedby={`${fieldId}-title-error`}
            onChange={(event) => setNewTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                event.preventDefault();
                void mutate('rename');
              }
            }}
          />
          {!titleValid ? (
            <small id={`${fieldId}-title-error`} className="chat-note">
              {t('titleInvalid')}
            </small>
          ) : null}
          <button
            type="button"
            className="button"
            disabled={!titleValid || Boolean(action)}
            onClick={() => void mutate('rename')}
          >
            {t('save')}
          </button>
          <button
            type="button"
            className="text-button"
            disabled={Boolean(action)}
            onClick={() => setRenaming(false)}
          >
            {t('cancel')}
          </button>
        </div>
      ) : null}
      {confirmDelete ? (
        <div className="chat-delete-confirm" role="group" aria-label={t('confirmDelete')}>
          <p>{t('confirmDelete')}</p>
          <button
            type="button"
            className="button"
            disabled={Boolean(action)}
            onClick={() => void mutate('delete')}
          >
            {t('deleteNow')}
          </button>
          <button
            type="button"
            className="text-button"
            disabled={Boolean(action)}
            onClick={() => setConfirmDelete(false)}
          >
            {t('cancel')}
          </button>
        </div>
      ) : null}
      {actionError ? <Failure code={actionError} language={language} onModels={onModels} /> : null}
      {notice ? (
        <p className="chat-note" role="status">
          {t(notice as ChatMessage)}
        </p>
      ) : null}
      <div className="chat-transcript" aria-busy={loading}>
        {loading ? (
          <p role="status" className="chat-note">
            {t('loading')}
          </p>
        ) : null}
        {loadError ? (
          <>
            <Failure code={loadError} language={language} onModels={onModels} />
            <button
              type="button"
              className="button"
              onClick={() => setRevision((value) => value + 1)}
            >
              {t('retry')}
            </button>
          </>
        ) : null}
        {view?.hasMore ? (
          <button
            type="button"
            className="button chat-older"
            disabled={loadingOlder || loading}
            onClick={() => void loadOlder()}
          >
            {t(loadingOlder ? 'loading' : 'older')}
          </button>
        ) : null}
        {view && !view.turns.length ? <p className="chat-empty">{t('emptyConversation')}</p> : null}
        <ol aria-label={view?.conversation.title ?? t('conversations')}>
          {view?.turns.map((turn) => (
            <Turn
              key={turn.id}
              turn={turn}
              language={language}
              onModels={onModels}
              onPreview={(current, citation) => void showPreview(current, citation)}
              onToggle={(current) => void toggleHidden(current)}
              toggling={toggling.includes(turn.id)}
            />
          ))}
        </ol>
      </div>
      {previewVisible ? (
        <dialog
          ref={previewDialog}
          className="chat-preview"
          aria-labelledby={`${fieldId}-preview`}
          aria-busy={previewLoading}
          onCancel={(event) => {
            event.preventDefault();
            closePreview();
          }}
        >
          <header>
            <h3 id={`${fieldId}-preview`}>{t('previewTitle')}</h3>
            <button
              type="button"
              className="icon-button small"
              aria-label={t('closePreview')}
              onClick={closePreview}
            >
              <X size={16} />
            </button>
          </header>
          {previewLoading ? <p role="status">{t('loading')}</p> : null}
          {previewError ? <p role="alert">{t('previewUnavailable')}</p> : null}
          {preview?.policyKey === policyKey ? (
            <>
              <strong>{basename(preview.chunk.filePath)}</strong>
              <code>{preview.chunk.filePath}</code>
              <p>
                {preview.chunk.page !== null
                  ? chatText(language, 'page', { value: preview.chunk.page })
                  : null}
                {preview.chunk.paragraph !== null
                  ? ' · ' + chatText(language, 'paragraph', { value: preview.chunk.paragraph })
                  : null}
              </p>
              <code>SHA-256 {preview.chunk.fingerprint.sha256}</code>
              {preview.chunk.heading ? <p className="chat-note">{preview.chunk.heading}</p> : null}
              <pre className="chat-plain">{preview.chunk.text}</pre>
              <button
                type="button"
                className="button"
                disabled={opening}
                onClick={() => void openPreview()}
              >
                {t('open')}
              </button>
            </>
          ) : null}
        </dialog>
      ) : null}
      {view?.selection ? (
        <SearchSelection
          selection={view.selection}
          language={language}
          disabled={busy || loading}
          onDirty={setSelectionDirty}
          onSave={async (selection) => {
            await rag.setSelection(id, selection);
            setPage((previous) =>
              previous ? { ...previous, value: { ...previous.value, selection } } : previous,
            );
            setSelectionDirty(false);
          }}
        />
      ) : null}
      <div className="chat-composer">
        <details className="chat-retrieval-options">
          <summary>
            {t('retrievalOptions')} · {t(mode)} ·{' '}
            {chatText(language, 'folderCount', { count: usableScopeIds.length })}
          </summary>
          <div className="chat-mode">
            <label htmlFor={`${fieldId}-mode`}>{t('mode')}</label>
            <select
              id={`${fieldId}-mode`}
              value={mode}
              onChange={(event) => setMode(event.target.value as 'documents' | 'general')}
            >
              <option value="documents">{t('documents')}</option>
              <option value="general">{t('general')}</option>
            </select>
          </div>
          {mode === 'general' ? (
            <p className="chat-note">{t('generalHelp')}</p>
          ) : (
            <fieldset className="chat-scope-picker">
              <legend>{t('selectedScopes')}</legend>
              {scopes.map((scope) => (
                <label key={scope.id} className="chat-check">
                  <input
                    type="checkbox"
                    checked={usableScopeIds.includes(scope.id)}
                    onChange={(event) =>
                      onScopes(
                        event.target.checked
                          ? [...usableScopeIds, scope.id]
                          : usableScopeIds.filter((value) => value !== scope.id),
                      )
                    }
                  />
                  <span>
                    {scope.name}
                    <small title={scope.path}>{scope.path}</small>
                    <small>{t(scope.externalAllowed ? 'externalYes' : 'externalNo')}</small>
                  </span>
                </label>
              ))}
              <p className="chat-note">{t('scopesHelp')}</p>
              {!usableScopeIds.length ? (
                <p className="chat-scope-warning" role="status">
                  {t('noScopes')}{' '}
                  <button type="button" className="text-button" onClick={onSettings}>
                    {t('scopes')}
                  </button>
                </p>
              ) : null}
            </fieldset>
          )}
          <p className="chat-note">{t(allowExternalQuestions ? 'questionsYes' : 'questionsNo')}</p>
          <p className="chat-note">{t('permissions')}</p>
          <label className="chat-check">
            <input
              type="checkbox"
              checked={sourcesHidden}
              onChange={(event) => setSourcesHidden(event.target.checked)}
            />
            <span>{t('hideSources')}</span>
          </label>
          <p className="chat-note">{t('hiddenHelp')}</p>
        </details>
        <label htmlFor={`${fieldId}-question`}>{t('question')}</label>
        <textarea
          id={`${fieldId}-question`}
          rows={3}
          value={question}
          maxLength={4096}
          placeholder={t('questionHint')}
          aria-describedby={`${fieldId}-question-limit`}
          aria-invalid={questionBytes > 4096}
          onChange={(event) => {
            questionVersion.current++;
            setQuestion(event.target.value);
          }}
          onKeyDown={(event) => {
            if (
              event.key === 'Enter' &&
              (event.ctrlKey || event.metaKey) &&
              !event.nativeEvent.isComposing &&
              event.keyCode !== 229
            ) {
              event.preventDefault();
              void submit();
            }
          }}
        />
        <p id={`${fieldId}-question-limit`} className="chat-note">
          {chatText(language, 'questionLimit', { bytes: questionBytes })}
        </p>
        {usableScopeIds.length > 128 ? (
          <p className="chat-scope-warning" role="status">
            {t('scopeLimit')}
          </p>
        ) : null}
        <div className="chat-send-row">
          <button
            type="button"
            className="button primary"
            disabled={!canSend}
            onClick={() => void submit()}
          >
            <Send size={15} />
            {t('send')}
          </button>
          {busy ? (
            <button
              type="button"
              className="button"
              disabled={cancelRequested}
              onClick={() => void cancel()}
            >
              <Square size={14} />
              {t('cancelGeneration')}
            </button>
          ) : null}
          <span role="status" className="chat-note">
            {cancelRequested && busy
              ? t('cancelRequested')
              : activeJob(job)
                ? t(job?.phase === 'generating' ? 'generating' : 'retrieving')
                : submitting
                  ? t('pending')
                  : null}
          </span>
        </div>
        {askError ? <Failure code={askError} language={language} onModels={onModels} /> : null}
      </div>
    </section>
  );
}

export function ChatView({
  onSettings,
  onModels,
  initialConversationId = null,
}: {
  onSettings: () => void;
  onModels?: () => void;
  initialConversationId?: string | null;
}) {
  const { settings, ready } = useClient();
  const language = settings.language;
  const t = (key: ChatMessage) => chatText(language, key);
  const searchId = useId();
  const scopes = settings.scopes.filter((scope) => scope.contentEnabled);
  const scopeKey = JSON.stringify(
    scopes.map((scope) => [scope.id, scope.path, scope.externalAllowed]),
  );
  const policyKey = JSON.stringify([settings.revision, scopeKey, settings.generation]);
  const [selectedScopeIds, setSelectedScopeIds] = useState<string[] | null>(null);
  const [query, setQuery] = useState('');
  const [list, setList] = useState<{ query: string; items: ConversationSummary[] } | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(initialConversationId);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listEpoch = useRef(0);
  const selectionEpoch = useRef(0);
  const createPending = useRef(false);
  const alive = useRef(false);
  const deleted = useRef(new Set<string>());
  const models = () => {
    if (onModels) onModels();
    else void bridge.openSettings('ai').catch(() => setError('worker'));
  };
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      listEpoch.current++;
      selectionEpoch.current++;
    };
  }, []);
  useEffect(() => {
    if (!ready) return;
    setSelectedScopeIds((previous) =>
      previous === null
        ? scopes.map((scope) => scope.id)
        : previous.filter((id) => scopes.some((scope) => scope.id === id)),
    );
  }, [ready, scopeKey]);
  useEffect(() => {
    if (!native) return;
    const epoch = ++listEpoch.current;
    let disposed = false;
    setLoading(true);
    setError(null);
    const timer = setTimeout(
      () => {
        void rag
          .list(query, 100)
          .then((items) => {
            if (!disposed && alive.current && epoch === listEpoch.current)
              setList({ query, items: items.filter((item) => !deleted.current.has(item.id)) });
          })
          .catch((cause) => {
            if (!disposed && alive.current && epoch === listEpoch.current)
              setError(failureCode(cause));
          })
          .finally(() => {
            if (!disposed && alive.current && epoch === listEpoch.current) setLoading(false);
          });
      },
      query ? 200 : 0,
    );
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [query, refresh]);
  const create = async () => {
    if (!native || createPending.current) return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    const selection = selectionEpoch.current;
    try {
      const summary = await rag.create(t('newTitle'));
      if (alive.current) {
        if (selectionEpoch.current === selection) {
          selectionEpoch.current++;
          setSelectedId(summary.id);
        }
        setRefresh((value) => value + 1);
      }
    } catch (cause) {
      if (alive.current) setError(failureCode(cause));
    } finally {
      createPending.current = false;
      if (alive.current) setCreating(false);
    }
  };
  const changed = () => setRefresh((value) => value + 1);
  const currentList =
    list?.query === query ? list.items.filter((item) => !deleted.current.has(item.id)) : [];
  return (
    <section className="feature-view chat-view">
      <div className="page-heading">
        <h1>{t('title')}</h1>
        <p>{t('description')}</p>
      </div>
      <div className="chat-top-actions">
        <button type="button" className="button" onClick={onSettings}>
          <FolderSearch size={16} />
          {t('scopes')}
        </button>
        <button type="button" className="button" onClick={models}>
          {t('models')}
        </button>
      </div>
      {!native ? (
        <div className="chat-native-only" role="status">
          <MessageSquareText size={48} strokeWidth={1.25} />
          <p>{t('nativeOnly')}</p>
        </div>
      ) : (
        <div className="chat-layout">
          <aside className="chat-sidebar" aria-labelledby={`${searchId}-title`}>
            <header>
              <h2 id={`${searchId}-title`}>{t('conversations')}</h2>
              <button
                type="button"
                className="icon-button small"
                disabled={loading}
                aria-label={t('refresh')}
                onClick={() => setRefresh((value) => value + 1)}
              >
                <RefreshCw size={15} />
              </button>
            </header>
            <label className="chat-sr-only" htmlFor={searchId}>
              {t('search')}
            </label>
            <input
              id={searchId}
              type="search"
              value={query}
              maxLength={256}
              placeholder={t('search')}
              onChange={(event) => setQuery(event.target.value)}
            />
            <button
              type="button"
              className="button"
              disabled={creating}
              onClick={() => void create()}
            >
              <Plus size={16} />
              {t(creating ? 'loading' : 'create')}
            </button>
            {loading ? (
              <p role="status" className="chat-note">
                {t('loading')}
              </p>
            ) : null}
            {error ? <Failure code={error} language={language} onModels={models} /> : null}
            {!loading && !currentList.length ? (
              <p className="chat-note">{t(query ? 'noMatches' : 'emptyList')}</p>
            ) : null}
            <ul className="chat-conversation-list">
              {currentList.map((item) => (
                <li key={item.id}>
                  <button
                    type="button"
                    aria-current={selectedId === item.id ? 'true' : undefined}
                    onClick={() => {
                      selectionEpoch.current++;
                      setSelectedId(item.id);
                    }}
                  >
                    <strong>{item.title || t('searchResultsTitle')}</strong>
                    <span>{chatText(language, 'turns', { count: item.turnCount })}</span>
                    <time dateTime={new Date(item.updatedMs).toISOString()}>
                      {new Date(item.updatedMs).toLocaleDateString(language)}
                    </time>
                  </button>
                </li>
              ))}
            </ul>
          </aside>
          {selectedId ? (
            <ConversationWorkspace
              key={selectedId}
              id={selectedId}
              language={language}
              scopes={scopes}
              selectedScopeIds={selectedScopeIds ?? []}
              onScopes={setSelectedScopeIds}
              policyKey={policyKey}
              allowExternalQuestions={settings.generation.allowExternalQuestions}
              onSettings={onSettings}
              onModels={models}
              onChanged={changed}
              onDeleted={() => {
                deleted.current.add(selectedId);
                selectionEpoch.current++;
                listEpoch.current++;
                setSelectedId(null);
                setRefresh((value) => value + 1);
              }}
            />
          ) : (
            <div className="chat-empty">
              <MessageSquareText size={44} strokeWidth={1.25} />
              <p>{t('selectConversation')}</p>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
