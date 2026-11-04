import { useEffect, useId, useState } from 'react';
import {
  BookOpen,
  ChevronDown,
  Copy,
  Download,
  ExternalLink,
  FolderInput,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react';
import { bridge, native } from '../../lib/bridge';
import { dictionary, dictionaryError } from '../../lib/dictionary';
import type {
  Definition,
  DictionaryController,
  DictionaryError,
  DictionaryLanguage,
  InstalledPack,
  PackJob,
} from '../../lib/dictionary';
import { setSession, showToast, useClient } from '../../lib/store';
import type { Language } from '../../lib/types';
import { dictText, dictionaryErrorText, dictionaryLanguageName } from './messages';
import './dictionary.css';

const languages: DictionaryLanguage[] = ['ko', 'en', 'zh', 'ja'];
const activePhases = new Set(['downloading', 'importing']);
function number(value: number, locale: Language) {
  return value.toLocaleString(locale);
}
function bytes(value: number, locale: Language) {
  const safe = Math.max(0, value);
  const power = Math.min(3, Math.floor(Math.log(Math.max(1, safe)) / Math.log(1024)));
  return `${(safe / 1024 ** power).toLocaleString(locale, { maximumFractionDigits: power ? 1 : 0 })} ${['B', 'KiB', 'MiB', 'GiB'][power]}`;
}
function Failure({ error, locale }: { error: DictionaryError; locale: Language }) {
  return (
    <p className="inline-error dictionary-error" role="alert">
      {dictionaryErrorText(locale, error.code)} <code>{error.code}</code>
    </p>
  );
}

function DefinitionCard({
  entry,
  locale,
  onAction,
  busy,
}: {
  entry: Definition;
  locale: Language;
  busy: boolean;
  onAction: (work: () => Promise<unknown>, copied?: boolean) => void;
}) {
  return (
    <li className="dictionary-definition">
      <header>
        <strong lang={entry.language}>{entry.term}</strong>
        <span className="muted" lang="en">
          {entry.partOfSpeech}
        </span>
        <button
          type="button"
          className="icon-button small"
          disabled={busy}
          aria-label={dictText(locale, 'copy')}
          title={dictText(locale, 'copy')}
          onClick={() => onAction(() => bridge.copyText(entry.text), true)}
        >
          <Copy size={16} />
        </button>
      </header>
      <p lang={entry.definitionLanguage}>{entry.text}</p>
      {entry.examples.length ? (
        <details className="dictionary-examples">
          <summary>
            {dictText(locale, 'examples', { count: number(entry.examples.length, locale) })}
          </summary>
          <ul>
            {entry.examples.map((example, index) => (
              <li key={index}>{example}</li>
            ))}
          </ul>
        </details>
      ) : null}
      <footer>
        <button
          type="button"
          className="text-button"
          disabled={busy}
          title={dictText(locale, 'source')}
          onClick={() => onAction(() => dictionary.openSource(entry.sourceUrl))}
        >
          {entry.sourceName}
          <ExternalLink size={12} />
        </button>
        <button
          type="button"
          className="text-button"
          disabled={busy}
          title={dictText(locale, 'license')}
          onClick={() => onAction(() => dictionary.openSource(entry.licenseUrl))}
        >
          {entry.license}
        </button>
        <small>
          {dictText(locale, 'definitionLanguage', {
            language: dictionaryLanguageName(locale, entry.definitionLanguage),
          })}
        </small>
      </footer>
    </li>
  );
}

function PackManager({ language, locale }: { language: DictionaryLanguage; locale: Language }) {
  const [snapshot, setSnapshot] = useState<{ packs: InstalledPack[]; job: PackJob } | null>(null);
  const [refreshError, setRefreshError] = useState<DictionaryError | null>(null);
  const [failure, setFailure] = useState<DictionaryError | null>(null);
  const [operation, setOperation] = useState<'download' | 'import' | 'remove' | null>(null);
  const [confirmation, setConfirmation] = useState<string | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [notice, setNotice] = useState<'installed' | 'removed' | null>(null);
  const [refresh, setRefresh] = useState(0);
  // These are local metadata reads. Word lookup and pack downloads never run in effects.
  useEffect(() => {
    let mounted = true;
    let pending = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const [packs, job] = await Promise.all([dictionary.packs(), dictionary.job()]);
        if (mounted) {
          setSnapshot({ packs, job });
          setRefreshError(null);
        }
      } catch (cause) {
        if (mounted) setRefreshError(dictionaryError(cause));
      } finally {
        pending = false;
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 1000);
    return () => {
      mounted = false;
      clearInterval(timer);
    };
  }, [refresh]);
  const job = snapshot?.job;
  const jobBusy = Boolean(job && activePhases.has(job.phase));
  const busy = operation !== null || jobBusy;
  const error = failure ?? refreshError ?? (!notice && !operation ? job?.error : null);
  const run = async (
    kind: 'download' | 'import' | 'remove',
    work: () => Promise<InstalledPack | null | void>,
  ) => {
    if (busy) return;
    setOperation(kind);
    setFailure(null);
    setNotice(null);
    setCancelRequested(false);
    try {
      const result = await work();
      if (kind !== 'import' || result) setNotice(kind === 'remove' ? 'removed' : 'installed');
      setConfirmation(null);
    } catch (cause) {
      setFailure(dictionaryError(cause));
    } finally {
      setOperation(null);
      setRefresh((value) => value + 1);
    }
  };
  const cancel = async () => {
    setCancelRequested(true);
    try {
      await dictionary.cancel();
    } catch (cause) {
      setFailure(dictionaryError(cause));
      setCancelRequested(false);
    }
  };
  const phase = jobBusy ? job!.phase : operation ? 'preparing' : job?.phase;
  return (
    <div className="dictionary-packs">
      {busy ? (
        <div className="dictionary-job" role="status" aria-live="polite">
          <div className="dictionary-job-heading">
            <strong>
              {dictText(
                locale,
                phase === 'downloading'
                  ? 'downloading'
                  : phase === 'importing'
                    ? 'importing'
                    : 'preparing',
              )}
            </strong>
            <button
              type="button"
              className="text-button"
              disabled={cancelRequested || operation === 'remove'}
              onClick={() => void cancel()}
            >
              {dictText(locale, cancelRequested ? 'cancelRequested' : 'cancel')}
            </button>
          </div>
          <progress
            aria-label={dictText(locale, 'progress')}
            max={job?.totalBytes && job.totalBytes > 0 ? job.totalBytes : 1}
            value={
              jobBusy && job?.totalBytes && job.totalBytes > 0
                ? Math.min(job.bytes, job.totalBytes)
                : undefined
            }
          />
          <small>
            {bytes(jobBusy ? (job?.bytes ?? 0) : 0, locale)} /{' '}
            {jobBusy && job?.totalBytes
              ? bytes(job.totalBytes, locale)
              : dictText(locale, 'unknownSize')}
            {' · '}
            {dictText(locale, 'entries', {
              count: number(jobBusy ? (job?.entries ?? 0) : 0, locale),
            })}
          </small>
        </div>
      ) : notice ? (
        <p className="dictionary-pack-notice" role="status">
          {dictText(locale, notice)}
        </p>
      ) : phase === 'cancelled' ? (
        <p className="dictionary-pack-notice" role="status">
          {dictText(locale, 'cancelled')}
        </p>
      ) : phase === 'ready' ? (
        <p className="dictionary-pack-notice" role="status">
          {dictText(locale, 'installed')}
        </p>
      ) : null}
      <div className="dictionary-pack-actions">
        <button
          type="button"
          className="button"
          disabled={busy || !snapshot}
          onClick={() => void run('download', () => dictionary.download(language))}
        >
          <Download size={15} />
          {dictText(locale, 'download', { language: dictionaryLanguageName(locale, language) })}
        </button>
        <button
          type="button"
          className="button"
          disabled={busy || !snapshot}
          onClick={() => void run('import', () => dictionary.importPack())}
        >
          <FolderInput size={15} />
          {dictText(locale, 'import')}
        </button>
        <button
          type="button"
          className="icon-button small"
          title={dictText(locale, 'refresh')}
          aria-label={dictText(locale, 'refresh')}
          onClick={() => setRefresh((value) => value + 1)}
        >
          <RefreshCw size={15} />
        </button>
      </div>
      <p className="dictionary-pack-hint">{dictText(locale, 'corpusWarning')}</p>
      <p className="dictionary-pack-hint">{dictText(locale, 'importHint')}</p>
      {error ? <Failure error={error} locale={locale} /> : null}
      <p className="dictionary-pack-hint">{dictText(locale, 'preserved')}</p>
      {!snapshot && !refreshError ? (
        <p role="status">{dictText(locale, 'packsLoading')}</p>
      ) : snapshot && !snapshot.packs.length ? (
        <p className="dictionary-pack-empty">{dictText(locale, 'packsEmpty')}</p>
      ) : null}
      <ul className="dictionary-pack-list">
        {snapshot?.packs.map((pack) => (
          <li key={pack.id}>
            <div className="dictionary-pack-name">
              <strong>{pack.manifest.name}</strong>
              <span>
                {dictionaryLanguageName(locale, pack.manifest.language)} ·{' '}
                {dictText(locale, 'entries', { count: number(pack.entries, locale) })}
              </span>
            </div>
            <div className="dictionary-pack-meta">
              <span>
                {dictText(locale, 'definitionLanguage', {
                  language: dictionaryLanguageName(locale, pack.manifest.definitionLanguage),
                })}
              </span>
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  void dictionary
                    .openSource(pack.manifest.sourceUrl)
                    .catch((cause) => setFailure(dictionaryError(cause)));
                }}
              >
                {dictText(locale, 'source')}
                <ExternalLink size={12} />
              </button>
              <button
                type="button"
                className="text-button"
                title={dictText(locale, 'license')}
                onClick={() => {
                  void dictionary
                    .openSource(pack.manifest.licenseUrl)
                    .catch((cause) => setFailure(dictionaryError(cause)));
                }}
              >
                {pack.manifest.license}
              </button>
            </div>
            {confirmation === pack.id ? (
              <div className="dictionary-remove-confirm" role="alert">
                <p>{dictText(locale, 'removeConfirm', { name: pack.manifest.name })}</p>
                <button
                  type="button"
                  className="button"
                  disabled={busy}
                  onClick={() => void run('remove', () => dictionary.remove(pack.id))}
                >
                  {dictText(locale, 'remove')}
                </button>
                <button
                  type="button"
                  className="text-button"
                  disabled={busy}
                  onClick={() => setConfirmation(null)}
                >
                  {dictText(locale, 'cancel')}
                </button>
              </div>
            ) : (
              <button
                type="button"
                className="text-button dictionary-remove"
                disabled={busy}
                onClick={() => setConfirmation(pack.id)}
              >
                <Trash2 size={13} />
                {dictText(locale, 'remove')}
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

export function DictionaryPanel({
  controller,
  compact = false,
}: {
  controller: DictionaryController;
  compact?: boolean;
}) {
  const { settings } = useClient();
  const locale = settings.language;
  const [managing, setManaging] = useState(false);
  const [actionError, setActionError] = useState<DictionaryError | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const packsId = useId();
  const selectId = useId();
  const result = controller.error ? null : controller.result;
  const run = async (work: () => Promise<unknown>, copied = false) => {
    setActionBusy(true);
    setActionError(null);
    try {
      await work();
      if (copied) showToast(dictText(locale, 'copied'));
    } catch (cause) {
      setActionError(dictionaryError(cause));
    } finally {
      setActionBusy(false);
    }
  };
  if (!native)
    return (
      <section
        className={`dictionary-panel ${compact ? 'compact' : ''}`}
        aria-label={dictText(locale, 'title')}
      >
        <header className="dictionary-heading">
          <BookOpen size={19} />
          <strong>{dictText(locale, 'title')}</strong>
        </header>
        <p className="dictionary-empty">{dictText(locale, 'nativeOnly')}</p>
      </section>
    );
  return (
    <section
      className={`dictionary-panel ${compact ? 'compact' : ''}`}
      aria-label={dictText(locale, 'title')}
    >
      <header className="dictionary-heading">
        <BookOpen size={19} />
        <strong>{dictText(locale, 'title')}</strong>
        <button
          type="button"
          className="text-button dictionary-pack-toggle"
          aria-expanded={managing}
          aria-controls={packsId}
          onClick={() => setManaging((value) => !value)}
        >
          {dictText(locale, 'packs')}
          <ChevronDown size={15} />
        </button>
      </header>
      <form
        className="dictionary-toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          if (!controller.loading) void controller.lookup();
        }}
      >
        <label htmlFor={selectId}>{dictText(locale, 'language')}</label>
        <select
          id={selectId}
          value={controller.language}
          onChange={(event) =>
            setSession({ query: `dict:${event.target.value} ${controller.word}` })
          }
        >
          {languages.map((language) => (
            <option key={language} value={language}>
              {dictionaryLanguageName(locale, language)}
            </option>
          ))}
        </select>
        <button
          type="submit"
          className="button primary"
          disabled={controller.loading || !controller.word.trim()}
        >
          <Search size={15} />
          {dictText(locale, 'lookup')}
        </button>
        <button
          type="button"
          className="text-button"
          disabled={controller.loading || !controller.word.trim()}
          onClick={() => void controller.lookup(true)}
        >
          {dictText(locale, 'offlineLookup')}
        </button>
      </form>
      <p className="dictionary-hint">
        {dictText(locale, 'submitHint')} {dictText(locale, 'englishHint')}
      </p>
      {managing ? (
        <div id={packsId}>
          <PackManager language={controller.language} locale={locale} />
        </div>
      ) : null}
      {controller.error ? <Failure error={controller.error} locale={locale} /> : null}
      {actionError ? <Failure error={actionError} locale={locale} /> : null}
      {controller.loading ? (
        <p className="dictionary-empty" role="status" aria-live="polite">
          {dictText(locale, 'loading')}
        </p>
      ) : result ? (
        <>
          <div className="dictionary-result-heading" role="status">
            <strong>{result.word}</strong>
            <span className={`dictionary-origin ${result.source}`}>
              {dictText(locale, result.source)}
            </span>
            <small>
              {dictText(locale, 'results', { count: number(result.definitions.length, locale) })}
            </small>
          </div>
          {result.source === 'offline' && result.offlineReason === 'network' ? (
            <p className="dictionary-hint">{dictText(locale, 'networkFallback')}</p>
          ) : null}
          {!result.definitions.length ? (
            <p className="dictionary-empty">
              {dictText(
                locale,
                result.source === 'offline' && !result.offlinePackInstalled ? 'noPack' : 'empty',
              )}
            </p>
          ) : null}
          <ol className="dictionary-definitions">
            {result.definitions.map((entry, index) => (
              <DefinitionCard
                key={`${entry.term}-${index}`}
                entry={entry}
                locale={locale}
                busy={actionBusy}
                onAction={(work, copied) => {
                  void run(work, copied);
                }}
              />
            ))}
          </ol>
        </>
      ) : !controller.error ? (
        <p className="dictionary-empty">{dictText(locale, 'ready')}</p>
      ) : null}
    </section>
  );
}
