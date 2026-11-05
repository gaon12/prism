import { useEffect, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import {
  AlertCircle,
  BrainCircuit,
  Copy,
  FileText,
  Folder,
  FolderOpen,
  Pause,
  Play,
  RefreshCw,
  Search,
  Settings2,
} from 'lucide-react';
import { bridge, native } from '../../lib/bridge';
import { content } from '../../lib/content';
import type { ContentController } from '../../lib/content';
import { translate } from '../../lib/i18n';
import { showToast, useClient } from '../../lib/store';
import { contentText } from './messages';
import { matchedSegments } from '../../lib/text-match';
import './content.css';
import { PdfUnlockDialog } from './PdfUnlockDialog';
import { IndexProgress } from '../../components/IndexProgress';

type MessageKey = Parameters<typeof contentText>[1];

function issueKey(code: string): MessageKey {
  switch (code.toLowerCase()) {
    case 'unsupportedformat':
    case 'unsupported':
      return 'unsupported';
    case 'encrypted':
      return 'encrypted';
    case 'corruptdocument':
    case 'damaged':
      return 'corrupt';
    case 'ocrrequired':
      return 'ocr';
    case 'ocrunavailable':
      return 'ocrUnavailable';
    case 'ocrlanguageunavailable':
      return 'ocrLanguageUnavailable';
    case 'ocrnotext':
      return 'ocrNoText';
    case 'ocrtimedout':
      return 'ocrTimedOut';
    case 'ocrbusy':
      return 'ocrBusy';
    case 'ocrfailed':
      return 'ocrFailed';
    case 'limitexceeded':
    case 'toolarge':
      return 'budget';
    case 'unsafepath':
      return 'unsafePath';
    case 'sourcechanged':
      return 'sourceChanged';
    case 'outsidescope':
      return 'outsideScope';
    case 'cancelled':
      return 'cancelled';
    case 'io':
      return 'io';
    default:
      return 'issue';
  }
}
function filename(path: string) {
  return path.split(/[\\/]/).pop() || path;
}
function failureMessage(cause: unknown) {
  return cause && typeof cause === 'object' && 'message' in cause
    ? String(cause.message)
    : cause instanceof Error
      ? cause.message
      : String(cause);
}

export function ContentResults({
  controller,
  compact = false,
  onSettings,
  onModels,
}: {
  controller: ContentController;
  compact?: boolean;
  onSettings: () => void;
  onModels: () => void;
}) {
  const { settings } = useClient();
  const language = settings.language;
  const t = (key: MessageKey, values?: Record<string, string | number>) =>
    contentText(language, key, values);
  const { data, status, selected } = controller;
  const automatic = controller.mode === 'auto';
  const semantic = controller.mode === 'semantic' || data?.automatic?.semanticState === 'used';
  const hybrid = data?.hybrid === true;
  const title = t(automatic ? 'automaticTitle' : semantic ? 'semanticTitle' : 'contentTitle');
  const list = useRef<HTMLDivElement>(null);
  const [busy, setBusy] = useState(false);
  const [unlockPath, setUnlockPath] = useState<string | null>(null);
  const [actionError, setActionError] = useState<{ key: string; message: string } | null>(null);
  const viewKey = JSON.stringify([
    controller.mode,
    settings.hybridSearch,
    settings.automaticSemantic,
    data?.automatic?.ranking,
    controller.query,
    controller.scopes.map((scope) => [scope.id, scope.path]),
  ]);
  const selectedId = selected?.chunk.id;
  const hasScopes = controller.scopes.length > 0;
  const indexing =
    status.phase === 'indexing' || status.phase === 'scanning' || status.phase === 'embedding';
  const paused = status.phase === 'cancelled' || status.phase === 'paused';
  const indexLabel = t(
    indexing ? 'indexing' : paused ? 'paused' : status.phase === 'error' ? 'indexError' : 'ready',
  );
  const hardLimitReached = data?.diagnostics.hardLimitReached === true;
  const automaticKeywordLimited =
    automatic && data?.keywordSkipped === true && data.automatic?.semanticState !== 'used';
  const queryEmpty = !controller.query.trim();
  const needsModel = semantic && Boolean(controller.error?.code.toLowerCase().includes('model'));
  const modelNotRunning = semantic && controller.error?.code.toLowerCase() === 'modelnotrunning';
  const numeric = new Intl.NumberFormat(language);
  const scores = new Intl.NumberFormat(language, { maximumFractionDigits: hybrid ? 6 : 3 });
  const visibleActionError = actionError?.key === viewKey ? actionError.message : null;

  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selectedId]);

  const run = async (work: () => Promise<unknown>, copied = false) => {
    if (!native || busy) return;
    setBusy(true);
    setActionError(null);
    try {
      await work();
      if (copied) showToast(translate(language, 'copied'));
    } catch (cause) {
      setActionError({ key: viewKey, message: failureMessage(cause) });
    } finally {
      setBusy(false);
    }
  };
  const openSelected = () => selected && void run(() => controller.open(selected.chunk.filePath));
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      event.nativeEvent.isComposing ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      !data?.hits.length
    )
      return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      event.stopPropagation();
      controller.move(event.key === 'ArrowDown' ? 1 : -1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      event.stopPropagation();
      openSelected();
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      event.stopPropagation();
      controller.select(data.hits[event.key === 'Home' ? 0 : data.hits.length - 1].chunk.id);
    }
  };

  if (!native)
    return (
      <section
        className={`content-panel content-native-only ${compact ? 'compact' : ''}`}
        aria-label={title}
      >
        {semantic ? (
          <BrainCircuit size={38} strokeWidth={1.4} aria-hidden="true" />
        ) : (
          <FileText size={38} strokeWidth={1.4} aria-hidden="true" />
        )}
        <h2>{title}</h2>
        <p>{translate(language, 'nativeOnly')}</p>
      </section>
    );

  return (
    <section className={`content-panel ${compact ? 'compact' : ''}`} aria-label={title}>
      {unlockPath ? (
        <PdfUnlockDialog
          key={unlockPath}
          path={unlockPath}
          language={language}
          onClose={() => setUnlockPath(null)}
        />
      ) : null}
      <header className="content-heading">
        {semantic ? (
          <BrainCircuit size={19} aria-hidden="true" />
        ) : (
          <FileText size={19} aria-hidden="true" />
        )}
        <strong>{title}</strong>
        <div className="content-toolbar">
          <button
            type="button"
            className="icon-button small"
            title={t('contentSettings')}
            aria-label={t('contentSettings')}
            onClick={onSettings}
          >
            <Settings2 size={17} />
          </button>
          <button
            type="button"
            className="icon-button small"
            title={t('models')}
            aria-label={t('models')}
            onClick={onModels}
          >
            <BrainCircuit size={17} />
          </button>
        </div>
      </header>
      <div className="content-scroll">
        <p className="content-scope-hint" title={t('scopeHint')}>
          {t('scopeCount', { count: numeric.format(controller.scopes.length) })}
        </p>
        {automatic && data?.automatic ? (
          <div className="content-alert automatic-search-notice" role="status">
            <div>
              <p>
                {t(
                  data.keywordSkipped && !data.hybrid
                    ? 'automaticKeywordLimit'
                    : data.automatic.semanticState === 'used'
                      ? 'automaticUsed'
                      : data.automatic.semanticState === 'disabled'
                        ? 'automaticDisabled'
                        : data.automatic.semanticState === 'failed'
                          ? 'automaticFailed'
                          : 'automaticNotRunning',
                )}
              </p>
              {data.automatic.semanticState !== 'used' ? (
                <button type="button" className="text-button" onClick={onModels}>
                  {t('models')}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}
        <div className="content-index" aria-label={indexLabel}>
          <span className={`content-index-state ${indexing ? 'busy' : ''}`} role="status">
            <span aria-hidden="true" />
            {indexLabel}
          </span>
          <div className="content-index-counts">
            <span>
              {t('indexed')} <b>{numeric.format(status.indexed)}</b>
            </span>
            <span>
              {t('discovered')} <b>{numeric.format(status.discovered)}</b>
            </span>
            <span>
              {t('skipped')} <b>{numeric.format(status.skipped)}</b>
            </span>
            <span>
              {t('embedded')} <b>{numeric.format(status.embedded)}</b>
            </span>
          </div>
          <div className="content-toolbar">
            {indexing ? (
              <button
                type="button"
                className="icon-button small"
                aria-label={t('pause')}
                title={t('pause')}
                disabled={busy}
                onClick={() => void run(content.pause)}
              >
                <Pause size={15} />
              </button>
            ) : null}
            <button
              type="button"
              className="icon-button small"
              aria-label={t(paused ? 'restart' : 'refresh')}
              title={t(paused ? 'restart' : 'refresh')}
              disabled={busy || !hasScopes || indexing}
              onClick={() => void run(content.refresh)}
            >
              {paused ? <Play size={15} /> : <RefreshCw size={15} />}
            </button>
          </div>
          <IndexProgress active={indexing} label={indexLabel} />
        </div>
        {status.currentPath ? (
          <p className="content-current-path" title={status.currentPath}>
            <span>{t('currentDocument')}</span>
            {status.currentPath}
          </p>
        ) : null}
        {controller.statusError ? (
          <div className="content-alert content-error" role="alert">
            <AlertCircle size={16} aria-hidden="true" />
            <div>
              <strong>{t('statusError')}</strong>
              <p>{controller.statusError.message}</p>
            </div>
          </div>
        ) : null}
        {visibleActionError ? (
          <div className="content-alert content-error" role="alert">
            <AlertCircle size={16} aria-hidden="true" />
            <p>{visibleActionError}</p>
          </div>
        ) : null}
        {status.embeddingError ? (
          <div className="content-alert content-embedding-alert">
            <BrainCircuit size={16} aria-hidden="true" />
            <div>
              <strong>{t('embeddingIssue')}</strong>
              <details>
                <summary>{t('details')}</summary>
                <p>{status.embeddingError}</p>
              </details>
            </div>
            <button type="button" className="text-button" onClick={onModels}>
              {t('models')}
            </button>
          </div>
        ) : null}
        {status.issues.length > 0 ? (
          <details className="content-issues">
            <summary>
              {t('issues')} · {numeric.format(status.issues.length)}
            </summary>
            <ul>
              {status.issues.map((issue, index) => (
                <li key={`${issue.path}:${issue.code}:${index}`}>
                  <div>
                    <strong>{t(issueKey(issue.code))}</strong>
                    <code>{issue.code}</code>
                  </div>
                  <p className="content-issue-path">{issue.path}</p>
                  {issueKey(issue.code) === 'issue' ? <p>{issue.message}</p> : null}
                  {issue.code.toLowerCase() === 'encrypted' && /\.pdf$/iu.test(issue.path) ? (
                    <button
                      type="button"
                      className="text-button"
                      aria-label={`${t('unlockPdf')}: ${filename(issue.path)}`}
                      onClick={() => setUnlockPath(issue.path)}
                    >
                      {t('unlockPdf')}
                    </button>
                  ) : null}
                </li>
              ))}
            </ul>
            {status.skipped > status.issues.length ? (
              <p className="content-issues-note">
                {t('issueLogLimited', {
                  shown: numeric.format(status.issues.length),
                  total: numeric.format(status.skipped),
                })}
              </p>
            ) : null}
          </details>
        ) : null}

        {hybrid && data ? (
          <p className="content-issues-note" role="status">
            {t(data.keywordSkipped ? 'hybridKeywordSkipped' : 'hybridActive')}
          </p>
        ) : null}
        {hardLimitReached && data ? (
          <div className="content-alert content-candidate-warning" role="status">
            <AlertCircle size={17} aria-hidden="true" />
            <div>
              <strong>{t('candidateLimit')}</strong>
              <p>
                {t(hybrid ? 'hybridCandidateLimitHint' : 'candidateLimitHint', {
                  count: numeric.format(data.diagnostics.candidateCount),
                  limit: numeric.format(data.diagnostics.candidateLimit),
                })}
              </p>
            </div>
          </div>
        ) : null}

        {!hasScopes ? (
          <div className="content-empty">
            <Folder size={compact ? 30 : 42} strokeWidth={1.4} aria-hidden="true" />
            <h2>{t('noScopes')}</h2>
            <p>{t('scopeHint')}</p>
            <button type="button" className="button" onClick={onSettings}>
              {t('chooseScopes')}
            </button>
          </div>
        ) : queryEmpty ? (
          <div className="content-empty">
            <Search size={compact ? 30 : 42} strokeWidth={1.4} aria-hidden="true" />
            <h2>{t(semantic ? 'emptySemantic' : 'emptyContent')}</h2>
            <p>{t('localSearch')}</p>
          </div>
        ) : controller.error ? (
          <div className="content-empty content-search-error" role="alert">
            <AlertCircle size={30} strokeWidth={1.5} aria-hidden="true" />
            <h2>{t(modelNotRunning ? 'modelNotRunning' : 'searchError')}</h2>
            <p>{controller.error.message}</p>
            {needsModel ? (
              <button type="button" className="button" onClick={onModels}>
                {t('models')}
              </button>
            ) : null}
          </div>
        ) : !data ? (
          <div className="content-empty" role="status">
            <Search size={28} strokeWidth={1.5} aria-hidden="true" />
            <p>{t('searching')}</p>
          </div>
        ) : data.hits.length === 0 ? (
          <div className="content-empty" role="status">
            <Search size={compact ? 30 : 42} strokeWidth={1.4} aria-hidden="true" />
            <h2>
              {t(
                automaticKeywordLimited
                  ? 'automaticKeywordLimitTitle'
                  : hardLimitReached
                    ? 'limitedEmpty'
                    : 'noHits',
              )}
            </h2>
            <p>
              {automaticKeywordLimited
                ? t('automaticKeywordLimit')
                : indexing
                  ? t('indexingHint')
                  : hardLimitReached
                    ? t('candidateLimit')
                    : t('changeQuery')}
            </p>
          </div>
        ) : (
          <div className="content-results">
            <div className="content-results-heading">
              <span>
                {t('results')} · {numeric.format(data.hits.length)}
                {controller.loading ? ` · ${t('searching')}` : ''}
              </span>
              <div className="content-toolbar">
                <button
                  type="button"
                  className="icon-button small"
                  aria-label={t('open')}
                  title={t('open')}
                  disabled={busy || !selected}
                  onClick={openSelected}
                >
                  <FolderOpen size={16} />
                </button>
                <button
                  type="button"
                  className="icon-button small"
                  aria-label={t('reveal')}
                  title={t('reveal')}
                  disabled={busy || !selected}
                  onClick={() =>
                    selected && void run(() => controller.open(selected.chunk.filePath, true))
                  }
                >
                  <Folder size={16} />
                </button>
                <button
                  type="button"
                  className="icon-button small"
                  aria-label={t('copyPath')}
                  title={t('copyPath')}
                  disabled={busy || !selected}
                  onClick={() =>
                    selected && void run(() => bridge.copyText(selected.chunk.filePath), true)
                  }
                >
                  <Copy size={15} />
                </button>
              </div>
            </div>
            <div
              id={controller.listId}
              ref={list}
              className="content-result-list"
              role="listbox"
              tabIndex={0}
              aria-label={t('results')}
              aria-busy={controller.loading}
              aria-activedescendant={selectedId ? `${controller.listId}-${selectedId}` : undefined}
              onKeyDown={keyDown}
            >
              {data.hits.map((hit) => (
                <div
                  key={hit.chunk.id}
                  id={`${controller.listId}-${hit.chunk.id}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={hit.chunk.id === selectedId}
                  className="content-result-row"
                  onClick={() => controller.select(hit.chunk.id)}
                  onDoubleClick={() => void run(() => controller.open(hit.chunk.filePath))}
                >
                  <FileText
                    size={compact ? 21 : 24}
                    strokeWidth={1.5}
                    className="content-file-icon"
                    aria-hidden="true"
                  />
                  <div className="content-result-main">
                    <strong className="content-result-filename">
                      {filename(hit.chunk.filePath)}
                    </strong>
                    <span className="content-result-path" title={hit.chunk.filePath}>
                      {hit.chunk.filePath}
                    </span>
                    {hit.chunk.heading ? (
                      <span className="content-result-section">{hit.chunk.heading}</span>
                    ) : null}
                    <p className="content-snippet">
                      {semantic && !hybrid
                        ? hit.snippet
                        : matchedSegments(hit.snippet, controller.query).map((part, index) =>
                            part.matched ? <mark key={index}>{part.text}</mark> : part.text,
                          )}
                    </p>
                    <div className="content-result-meta">
                      {hit.chunk.page !== null ? (
                        <span>
                          {t('page')} {numeric.format(hit.chunk.page)}
                        </span>
                      ) : null}
                      {hit.chunk.paragraph !== null ? (
                        <span>
                          {t('paragraph')} {numeric.format(hit.chunk.paragraph)}
                        </span>
                      ) : null}
                      {hybrid ? (
                        <span>
                          {t(
                            hit.keywordRank
                              ? hit.semanticRank
                                ? 'hybridBothMatch'
                                : 'hybridKeywordMatch'
                              : 'hybridSemanticMatch',
                          )}
                        </span>
                      ) : null}
                      <span
                        className="content-score"
                        title={t(
                          hybrid
                            ? 'hybridScoreHint'
                            : semantic
                              ? 'cosineScoreHint'
                              : 'keywordScoreHint',
                        )}
                      >
                        {t(hybrid ? 'hybridScore' : semantic ? 'cosineScore' : 'keywordScore')}{' '}
                        {Number.isFinite(hit.score) ? scores.format(hit.score) : '—'}
                      </span>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
