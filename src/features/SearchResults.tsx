import { useEffect, useId, useRef, useState } from 'react';
import {
  FileText,
  Folder,
  FolderOpen,
  Copy,
  RefreshCw,
  Calculator,
  ArrowRightLeft,
  Globe,
  Search,
  AlertCircle,
} from 'lucide-react';
import { setSession } from '../lib/store';
import { native } from '../lib/bridge';
import { workspaceText } from '../components/workspace-messages';
import { providerNames } from '../lib/utilities';
import type { SearchController } from '../lib/use-search';
import { EmptyState } from '../components/EmptyState';
import { IndexProgress } from '../components/IndexProgress';
import { drawnRows, parentFolder, RESULT_PAGE } from '../lib/result-window';
import './results.css';

export function UtilityResult({ search }: { search: SearchController }) {
  const accuracyId = useId();
  const { request, utility, t } = search;
  const kind =
    request.kind === 'calculator' ? 'calculator' : request.kind === 'units' ? 'units' : 'web';
  const Icon = kind === 'calculator' ? Calculator : kind === 'units' ? ArrowRightLeft : Globe;
  return (
    <section className="utility-result" aria-label={t(kind)}>
      <div className="utility-heading">
        <Icon size={20} />
        <strong>{t(kind)}</strong>
        {kind === 'web' ? <span>{providerNames[request.provider!]}</span> : null}
      </div>
      {!request.input ? (
        <p className="utility-help">
          {kind === 'calculator'
            ? t('enterExpression')
            : kind === 'units'
              ? t('enterUnit')
              : t('enterWeb')}
        </p>
      ) : kind === 'web' ? (
        <>
          <p className="web-query">{request.input}</p>
          <button className="button primary" onClick={search.activate}>
            <Globe size={16} />
            {t('openBrowser')}
          </button>
          <p className="utility-help">{t('explicitWeb')}</p>
        </>
      ) : utility?.error ? (
        <p className="inline-error" role="status">
          <AlertCircle size={17} />
          {utility.error === 'timeout'
            ? t('calculationTimeout')
            : kind === 'calculator'
              ? t('invalidCalculation')
              : t('invalidUnit')}
        </p>
      ) : utility?.answer ? (
        <>
          <output
            className="utility-value"
            aria-live="polite"
            aria-describedby={utility.answer.accuracy ? accuracyId : undefined}
          >
            {utility.answer.value}
          </output>
          {utility.answer.accuracy ? (
            <p id={accuracyId} className="accuracy-note">
              {t(utility.answer.accuracy.rounded ? 'roundedValue' : 'approximateValue').replace(
                '{digits}',
                String(utility.answer.accuracy.significantDigits),
              )}
            </p>
          ) : null}
          {utility.answer.interpretation ? (
            <p className="interpreted-expression">
              {t('interpretedExpression')}: <code>{utility.answer.interpretation}</code>
            </p>
          ) : null}
          {utility.answer.exact ? (
            <p className="exact-value">
              {t('exactValue')}: {utility.answer.exact}
            </p>
          ) : null}
          <button className="button" onClick={() => search.copy(utility.answer!.value)}>
            <Copy size={16} />
            {t('copy')}
          </button>
          <p className="utility-help">
            {kind === 'calculator'
              ? t('numericHint')
              : '1 GB = 1000 MB · 1 GiB = 1024 MiB · mpg = US'}
          </p>
        </>
      ) : (
        <p className="utility-help" role="status">
          {t('computing')}
        </p>
      )}
    </section>
  );
}

export function IndexSummary({ search }: { search: SearchController }) {
  const { t, status } = search;
  const label = status.scanning
    ? workspaceText(search.language, status.initialScanComplete ? 'updating' : 'collecting')
    : t('indexed');
  if (
    !native ||
    search.request.kind !== 'files' ||
    (search.bodyActive && search.session.mode !== 'auto')
  )
    return null;
  return (
    <div className="index-summary">
      <span>
        <span className={`index-dot ${status.scanning ? 'busy' : ''}`} />
        {label} · {status.count.toLocaleString()}
      </span>
      <button
        className="text-button"
        title={t('refresh')}
        aria-label={t('refresh')}
        onClick={search.refresh}
        disabled={status.scanning}
      >
        <RefreshCw size={14} />
      </button>
      {status.skipped > 0 ? (
        <span title={t('skipped')}>
          {t('skipped')}: {status.skipped}
        </span>
      ) : null}
      <IndexProgress active={status.scanning} label={label} />
    </div>
  );
}

export function FileResults({
  search,
  quick = false,
}: {
  search: SearchController;
  quick?: boolean;
}) {
  const { t, data, selected, error, loading, status, session, language } = search;
  const list = useRef<HTMLDivElement>(null);
  const end = useRef<HTMLDivElement>(null);
  // Rows opened by scrolling (see result-window.ts). A new query starts from
  // one page again; a refresh of the same query keeps what was opened.
  const [shown, setShown] = useState(RESULT_PAGE);
  useEffect(() => setShown(RESULT_PAGE), [session.query]);
  const results = data?.results ?? [];
  const drawn = drawnRows(
    results.length,
    shown,
    results.findIndex((file) => file.path === selected?.path),
  );
  // Draw another page when the end of the drawn rows comes within a screen
  // of view. Watching visibility, not one element's scroll position, works
  // whichever container scrolls: the main window scrolls an outer pane, the
  // quick window the list itself. A new observer reports at once, so a tall
  // window is filled page by page until its end is out of view.
  useEffect(() => {
    const element = end.current;
    if (!element || drawn >= results.length || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setShown(drawn + RESULT_PAGE);
      },
      { rootMargin: '0px 0px 100% 0px' },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [drawn, results.length]);
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selected?.path]);
  if (!native)
    return (
      <EmptyState icon={<Search size={44} />} title={t('nativeOnly')}>
        {null}
      </EmptyState>
    );
  if (error)
    return (
      <div className="result-message inline-error" role="status">
        <AlertCircle size={20} />
        <p>{error}</p>
      </div>
    );
  if (loading && !data)
    return (
      <div className="result-message" role="status">
        <Search size={24} />
        {t('searching')}
      </div>
    );
  if (!data?.total)
    return (
      <EmptyState
        icon={<Search size={quick ? 42 : 60} strokeWidth={1.4} />}
        title={
          status.scanning
            ? workspaceText(language, status.initialScanComplete ? 'updating' : 'collecting')
            : t('noResults')
        }
      >
        <p>{status.scanning ? t('searching') : t('changeQuery')}</p>
      </EmptyState>
    );
  return (
    <div className={`file-results ${quick ? 'compact-results' : ''}`}>
      <div className="results-heading">
        <span>
          {t('results')} · {data.total.toLocaleString()}
          {loading ? ` · ${t('searching')}` : ''}
        </span>
        <div className="result-actions">
          <button
            className="icon-button small"
            aria-label={t('open')}
            title={t('open')}
            onClick={search.activate}
          >
            <FolderOpen size={16} />
          </button>
          <button
            className="icon-button small"
            aria-label={t('reveal')}
            title={t('reveal')}
            onClick={search.reveal}
          >
            <Folder size={16} />
          </button>
          <button
            className="icon-button small"
            aria-label={t('copyPath')}
            title={t('copyPath')}
            onClick={() => selected && search.copy(selected.path)}
          >
            <Copy size={15} />
          </button>
        </div>
      </div>
      {data.keyboardCorrected ? (
        <p className="keyboard-notice" role="status">
          {t('keyboardCorrectedNotice')}
        </p>
      ) : null}
      {/* Column names. Hidden from assistive technology: each option is read as a whole. */}
      <div className="file-result-columns" aria-hidden="true">
        <span>{t('columnName')}</span>
        <span>{t('folder')}</span>
        {!quick ? (
          <>
            <span>{t('columnSize')}</span>
            <span>{t('modified')}</span>
          </>
        ) : null}
      </div>
      <div
        id={search.listId}
        className="file-result-list"
        role="listbox"
        aria-label={t('results')}
        aria-activedescendant={search.selectedId}
        tabIndex={0}
        onKeyDown={search.keyDown}
        ref={list}
      >
        {data.results.slice(0, drawn).map((file, index) => (
          <div
            key={file.path}
            id={`${search.listId}-${index}`}
            role="option"
            // Assistive technology still hears the full count.
            aria-setsize={data.results.length}
            aria-posinset={index + 1}
            tabIndex={-1}
            aria-selected={file.path === selected?.path}
            className="file-result-row"
            onClick={() => {
              setSession({ selectedPath: file.path });
              list.current?.focus({ preventScroll: true });
            }}
            onDoubleClick={() => search.openFile(file.path)}
          >
            <span className="file-result-name">
              {file.isDir ? (
                <Folder className="file-icon" size={16} strokeWidth={1.6} />
              ) : (
                <FileText className="file-icon" size={16} strokeWidth={1.6} />
              )}
              <span className="file-result-title">{highlight(file.name, session.query)}</span>
            </span>
            <span className="file-result-folder" title={file.path}>
              {parentFolder(file.path, file.name)}
            </span>
            {!quick ? (
              <>
                <span className="file-result-size">
                  {file.isDir ? t('folder') : formatSize(file.size)}
                </span>
                <time className="file-result-date">
                  {file.modifiedMs ? new Date(file.modifiedMs).toLocaleDateString(language) : '—'}
                </time>
              </>
            ) : null}
          </div>
        ))}
        {drawn < data.results.length ? (
          <div ref={end} className="results-more" aria-hidden="true" />
        ) : null}
      </div>
      {data.total > data.results.length ? <p className="results-limit">{t('showLimit')}</p> : null}
    </div>
  );
}
function formatSize(size: number) {
  if (size < 1024) return `${size} B`;
  if (size < 1024 ** 2) return `${(size / 1024).toFixed(1)} KiB`;
  return `${(size / 1024 ** 2).toFixed(1)} MiB`;
}
function highlight(name: string, query: string) {
  const term = query
    .trim()
    .split(/\s+/)
    .find((term) => !term.includes(':'))
    ?.replaceAll('"', '');
  if (!term) return name;
  const offset = name.toLowerCase().indexOf(term.toLowerCase());
  if (offset < 0) return name;
  return (
    <>
      {name.slice(0, offset)}
      <mark>{name.slice(offset, offset + term.length)}</mark>
      {name.slice(offset + term.length)}
    </>
  );
}
