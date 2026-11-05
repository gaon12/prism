import { useEffect, useId, useRef } from 'react';
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
import { providerNames } from '../lib/utilities';
import type { SearchController } from '../lib/use-search';
import { EmptyState } from '../components/EmptyState';
import { IndexProgress } from '../components/IndexProgress';
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
        {t(status.scanning ? 'indexing' : 'indexed')} · {status.count.toLocaleString()}
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
      <IndexProgress active={status.scanning} label={t('indexing')} />
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
        title={status.scanning ? t('indexing') : t('noResults')}
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
      <div
        id={search.listId}
        className="file-result-list"
        role="listbox"
        aria-label={t('results')}
        ref={list}
      >
        {data.results.map((file, index) => (
          <div
            key={file.path}
            id={`${search.listId}-${index}`}
            role="option"
            tabIndex={-1}
            aria-selected={file.path === selected?.path}
            className="file-result-row"
            onClick={() => setSession({ selectedPath: file.path })}
            onDoubleClick={() => search.openFile(file.path)}
          >
            {file.isDir ? (
              <Folder className="file-icon" size={quick ? 22 : 25} strokeWidth={1.5} />
            ) : (
              <FileText className="file-icon" size={quick ? 22 : 25} strokeWidth={1.5} />
            )}
            <div className="file-result-name">
              <strong>{highlight(file.name, session.query)}</strong>
              <span title={file.path}>{file.path}</span>
            </div>
            {!quick ? (
              <div className="file-result-details">
                <span>{file.isDir ? t('folder') : formatSize(file.size)}</span>
                <time>
                  {file.modifiedMs ? new Date(file.modifiedMs).toLocaleDateString(language) : '—'}
                </time>
              </div>
            ) : null}
          </div>
        ))}
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
