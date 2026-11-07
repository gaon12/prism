import { useEffect, useId, useRef, useState } from 'react';
import { Cpu, Download, ExternalLink, FolderInput, Play, RefreshCw, Square, X } from 'lucide-react';
import { native } from '../../lib/bridge';
import { embeddingAssets, model } from '../../lib/model';
import type { EmbeddingStatus } from '../../lib/model';
import { errorMessage, useClient } from '../../lib/store';
import type { Language } from '../../lib/types';
import { modelErrorText, modelText } from './messages';
import './models.css';

type Operation = 'download' | 'model' | 'runtime' | 'start' | 'stop';
function formatBytes(value: number, locale: Language) {
  const bytes = Math.max(0, value);
  const power = Math.min(3, Math.floor(Math.log(Math.max(1, bytes)) / Math.log(1024)));
  return `${(bytes / 1024 ** power).toLocaleString(locale, { maximumFractionDigits: power ? 1 : 0 })} ${['B', 'KiB', 'MiB', 'GiB'][power]}`;
}
function ModelFailure({ message, locale }: { message: string; locale: Language }) {
  return (
    <div className="embedding-error" role={/cancelled/i.test(message) ? 'status' : 'alert'}>
      <p>{modelErrorText(locale, message)}</p>
      <details>
        <summary>{modelText(locale, 'details')}</summary>
        <pre lang="en">{message}</pre>
      </details>
    </div>
  );
}

/** Safe to place inside SettingsDialog's form: every action is type="button". */
export function EmbeddingPanel() {
  const { settings } = useClient();
  const locale = settings.language;
  const titleId = useId();
  const [status, setStatus] = useState<EmbeddingStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [operation, setOperation] = useState<Operation | null>(null);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const statusEpoch = useRef(0);
  // Poll local state only. Downloading, importing and starting require a click.
  useEffect(() => {
    if (!native) return;
    let mounted = true;
    let pending = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      const before = statusEpoch.current;
      try {
        const next = await model.status();
        if (mounted && before === statusEpoch.current) {
          setStatus(next);
          setStatusError(null);
        }
      } catch (cause) {
        if (mounted && before === statusEpoch.current) setStatusError(errorMessage(cause));
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
  const working = status?.phase === 'downloading' || status?.phase === 'starting';
  const busy = Boolean(operation || working);
  const running = status?.phase === 'running' || status?.pid != null;
  const mutable = Boolean(status?.runtimeTarget && !busy && !running && !statusError);
  const error = actionError ?? statusError ?? (!operation ? status?.lastError : null);
  const run = async (kind: Operation, work: () => Promise<EmbeddingStatus | null>) => {
    if (busy) return;
    statusEpoch.current++;
    setOperation(kind);
    setActionError(null);
    setCancelRequested(false);
    try {
      const next = await work();
      if (next) {
        setStatus(next);
        setStatusError(null);
      }
    } catch (cause) {
      setActionError(errorMessage(cause));
    } finally {
      // A poll can settle after this action but before its effect cleanup.
      // Reject those older reads, including errors, before refreshing status.
      statusEpoch.current++;
      setOperation(null);
      setRefresh((value) => value + 1);
    }
  };
  const cancel = async () => {
    setCancelRequested(true);
    setActionError(null);
    try {
      await model.cancel();
    } catch (cause) {
      setActionError(errorMessage(cause));
      setCancelRequested(false);
    }
  };
  const openSource = async (url: string) => {
    try {
      await model.openSource(url);
    } catch (cause) {
      setActionError(errorMessage(cause));
    }
  };
  const preparingFiles =
    status?.phase === 'downloading' ||
    operation === 'download' ||
    operation === 'model' ||
    operation === 'runtime';
  return (
    <section className="embedding-panel" aria-labelledby={titleId}>
      <header className="embedding-heading">
        <Cpu size={20} />
        <h3 id={titleId}>{modelText(locale, 'title')}</h3>
      </header>
      <p className="embedding-description">{modelText(locale, 'description')}</p>
      {!native ? (
        <p className="embedding-note" role="status">
          {modelText(locale, 'nativeOnly')}
        </p>
      ) : (
        <>
          <div className="embedding-status-heading">
            <code>{status?.modelId ?? embeddingAssets.modelId}</code>
            <span
              className="embedding-phase"
              data-phase={statusError ? 'unknown' : (status?.phase ?? 'unknown')}
              role="status"
            >
              {statusError
                ? modelText(locale, 'unknown')
                : status
                  ? modelText(locale, status.phase)
                  : modelText(locale, 'loading')}
            </span>
            <button
              type="button"
              className="icon-button small"
              title={modelText(locale, 'refresh')}
              aria-label={modelText(locale, 'refresh')}
              onClick={() => setRefresh((value) => value + 1)}
            >
              <RefreshCw size={15} />
            </button>
          </div>
          {status && !status.runtimeTarget ? (
            <p className="embedding-note" role="status">
              {modelText(locale, 'unsupportedPlatform')}
            </p>
          ) : null}
          <dl className="embedding-metadata">
            <div>
              <dt>{modelText(locale, 'modelFile')}</dt>
              <dd>
                {status ? modelText(locale, status.modelReady ? 'installed' : 'notInstalled') : '—'}
              </dd>
            </div>
            <div>
              <dt>{modelText(locale, 'cpuRuntime')}</dt>
              <dd>
                {status
                  ? modelText(locale, status.runtimeReady ? 'installed' : 'notInstalled')
                  : '—'}{' '}
                <small>{status?.runtimeVersion ?? embeddingAssets.runtimeVersion}</small>
              </dd>
            </div>
            <div>
              <dt>{modelText(locale, 'quantization')}</dt>
              <dd>{status?.quantization ?? '—'}</dd>
            </div>
            <div>
              <dt>{modelText(locale, 'dimensions')}</dt>
              <dd>{status?.dimensions.toLocaleString(locale) ?? '—'}</dd>
            </div>
          </dl>
          {busy ? (
            <div className="embedding-progress">
              <p role="status">
                {cancelRequested
                  ? modelText(locale, 'cancelRequested')
                  : modelText(
                      locale,
                      operation === 'stop'
                        ? 'stopping'
                        : status?.phase === 'starting' || operation === 'start'
                          ? 'starting'
                          : 'preparing',
                    )}
              </p>
              {preparingFiles ? (
                <>
                  <progress
                    aria-label={modelText(locale, 'progress')}
                    max={status?.totalBytes && status.totalBytes > 0 ? status.totalBytes : 1}
                    value={
                      status?.phase === 'downloading' && status.totalBytes > 0
                        ? Math.min(status.downloadedBytes, status.totalBytes)
                        : undefined
                    }
                  />
                  <small className="embedding-progress-bytes">
                    {formatBytes(
                      status?.phase === 'downloading' ? status.downloadedBytes : 0,
                      locale,
                    )}{' '}
                    /{' '}
                    {status?.phase === 'downloading' && status.totalBytes > 0
                      ? formatBytes(status.totalBytes, locale)
                      : modelText(locale, 'unknownTotal')}
                  </small>
                </>
              ) : null}
              {operation !== 'stop' ? (
                <div className="embedding-progress-actions">
                  <button
                    type="button"
                    className="text-button"
                    disabled={cancelRequested}
                    onClick={() => void cancel()}
                  >
                    <X size={14} />
                    {modelText(locale, cancelRequested ? 'cancelRequested' : 'cancel')}
                  </button>
                </div>
              ) : null}
            </div>
          ) : null}
          {error ? <ModelFailure message={error} locale={locale} /> : null}
          <div className="embedding-actions">
            <button
              type="button"
              className="button primary"
              disabled={!mutable || !status?.modelReady || !status.runtimeReady}
              onClick={() => void run('start', () => model.start())}
            >
              <Play size={15} />
              {modelText(locale, 'start')}
            </button>
            <button
              type="button"
              className="button"
              disabled={!running || busy}
              onClick={() => void run('stop', () => model.stop())}
            >
              <Square size={14} />
              {modelText(locale, 'stop')}
            </button>
          </div>
          <p className="embedding-note">{modelText(locale, running ? 'stopHint' : 'startHint')}</p>
          <div className="embedding-install">
            <button
              type="button"
              className="button"
              disabled={!mutable}
              onClick={() => void run('download', () => model.download())}
            >
              <Download size={15} />
              {modelText(locale, 'download')}
            </button>
            <p className="embedding-note">{modelText(locale, 'downloadSize')}</p>
          </div>
          <div className="embedding-offline">
            <h4>{modelText(locale, 'offlineImport')}</h4>
            <p className="embedding-note">{modelText(locale, 'importHint')}</p>
            <div className="embedding-actions">
              <button
                type="button"
                className="button"
                disabled={!mutable}
                onClick={() =>
                  void run('model', () => model.importModel(modelText(locale, 'chooseModel')))
                }
              >
                <FolderInput size={15} />
                {modelText(locale, 'importModel')}
              </button>
              <button
                type="button"
                className="button"
                disabled={!mutable}
                onClick={() =>
                  void run('runtime', () => model.importRuntime(modelText(locale, 'chooseRuntime')))
                }
              >
                <FolderInput size={15} />
                {modelText(locale, 'importRuntime')}
              </button>
            </div>
          </div>
          <p className="embedding-note">{modelText(locale, 'verification')}</p>
          <details className="embedding-files">
            <summary>{modelText(locale, 'accepted')}</summary>
            <ul>
              {embeddingAssets.modelFiles.map((file) => (
                <li key={file}>
                  <code>{file}</code>
                </li>
              ))}
              <li>
                <code>{status?.runtimeFilename ?? modelText(locale, 'unknown')}</code>
              </li>
            </ul>
            {status ? (
              <p>
                {modelText(locale, 'revision')}: <code>{status.modelRevision}</code>
              </p>
            ) : null}
          </details>
          <nav className="embedding-links" aria-label={modelText(locale, 'accepted')}>
            {(
              [
                ['officialModel', embeddingAssets.modelUrl],
                ['officialGguf', embeddingAssets.ggufUrl],
                ['officialRuntime', embeddingAssets.runtimeUrl],
              ] as const
            ).map(([label, url]) => (
              <button
                key={label}
                type="button"
                className="text-button"
                onClick={() => void openSource(url)}
              >
                {modelText(locale, label)}
                <ExternalLink size={12} />
              </button>
            ))}
          </nav>
        </>
      )}
      <p className="embedding-note embedding-platform">{modelText(locale, 'platform')}</p>
      {native ? <p className="embedding-note">{modelText(locale, 'local')}</p> : null}
    </section>
  );
}
