import { useEffect, useId, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { native } from '../../lib/bridge';
import type { Language } from '../../lib/types';
import { libraryText, failureKey } from './messages';
import { officeEngine, type OfficeStatus } from './office-engine';
import { officeText } from './office-engine-messages';
import './office-engine.css';

/** Progress stays local to this panel, never remounting the settings form. */
export function OfficeEnginePanel({ language }: { language: Language }) {
  const title = useId();
  const alive = useRef(false);
  const [status, setStatus] = useState<OfficeStatus | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [action, setAction] = useState(false);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [actionError, setActionError] = useState<{ cause: unknown; revision: number } | null>(null);
  const t = (key: Parameters<typeof officeText>[1], values?: Record<string, string | number>) =>
    officeText(language, key, values);
  const accept = (next: OfficeStatus) => {
    if (alive.current) {
      setStatus((current) => (!current || next.revision >= current.revision ? next : current));
      setFailed(false);
      if (!next.installing) setCancelRequested(false);
    }
  };
  useEffect(() => {
    alive.current = true;
    if (!native)
      return () => {
        alive.current = false;
      };
    let disposed = false;
    let stop: (() => void) | undefined;
    // Listen before reading the retained snapshot. Revision resolves in-flight
    // reads versus events, including install completion while this is mounting.
    void listen<OfficeStatus>('prism:office-status', (event) => {
      if (!disposed) accept(event.payload);
    })
      .then(async (unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        stop = unlisten;
        const next = await officeEngine.status();
        if (!disposed) accept(next);
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });
    return () => {
      disposed = true;
      alive.current = false;
      stop?.();
    };
  }, [attempt]);
  const install = async () => {
    setAction(true);
    setActionError(null);
    setCancelRequested(false);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        filters: [{ name: 'Prism Office ZIP', extensions: ['zip'] }],
      });
      if (typeof path !== 'string' || !alive.current) return;
      await officeEngine.install(path);
    } catch (cause) {
      if (alive.current) setActionError({ cause, revision: status?.revision ?? 0 });
    } finally {
      if (alive.current) {
        setAction(false);
        void officeEngine.status().then(accept, () => {
          if (alive.current) setFailed(true);
        });
      }
    }
  };
  const cancel = async () => {
    setCancelRequested(true);
    setActionError(null);
    try {
      await officeEngine.cancel();
    } catch (cause) {
      if (alive.current) {
        setActionError({ cause, revision: status?.revision ?? 0 });
        setCancelRequested(false);
      }
    }
  };
  // Another window can start a newer job. Its result supersedes errors from
  // this panel's older action, even if this panel stayed mounted throughout.
  const issue =
    actionError && actionError.revision >= (status?.revision ?? 0)
      ? actionError.cause
      : status?.lastError
        ? { code: status.lastError }
        : null;
  const cancelled = issue && failureKey(issue) === 'cancelled';
  const busy = status?.installing ?? false;
  const progress = status?.progress;
  const bytes = (value: number) =>
    `${(value / 1024 ** 2).toLocaleString(language, { maximumFractionDigits: 0 })} MiB`;
  return (
    <section className="office-engine-panel" aria-labelledby={title}>
      <h3 id={title}>{t('title')}</h3>
      <p>{t('description')}</p>
      {!native ? (
        <p className="muted">{t('desktop')}</p>
      ) : failed ? (
        <div role="alert">
          <p>{t('failed')}</p>
          <button type="button" className="button" onClick={() => setAttempt((n) => n + 1)}>
            {t('retry')}
          </button>
        </div>
      ) : !status ? (
        <p role="status">{t('loading')}</p>
      ) : !status.supported ? (
        <p className="muted">{t('unsupported')}</p>
      ) : (
        <>
          <p className="office-engine-presence">{t(status.installed ? 'present' : 'absent')}</p>
          <dl className="office-engine-capacity">
            <div>
              <dt>{t('archive')}</dt>
              <dd>{bytes(status.archiveBytes)}</dd>
            </div>
            <div>
              <dt>{t('disk')}</dt>
              <dd>{bytes(status.expandedBytes)}</dd>
            </div>
          </dl>
          {!status.downloadAvailable ? (
            <p className="setting-footnote">{t('unpublished')}</p>
          ) : null}
          <p className="setting-footnote">{t('verify')}</p>
          <div className="office-engine-job" aria-live="polite">
            {busy ? (
              <>
                <p>{t(progress?.phase === 'waiting' ? 'waiting' : 'installing')}</p>
                <progress
                  aria-label={t('installing')}
                  max={progress?.totalFiles || 1}
                  value={
                    progress?.totalFiles && progress.files > 0
                      ? Math.min(progress.files, progress.totalFiles)
                      : undefined
                  }
                />
                {progress?.totalFiles ? (
                  <small>
                    {t('files', {
                      n: progress.files.toLocaleString(language),
                      total: progress.totalFiles.toLocaleString(language),
                    })}
                  </small>
                ) : null}
              </>
            ) : issue ? (
              <p role={cancelled ? 'status' : 'alert'}>
                {cancelled ? t('cancelled') : libraryText(language, failureKey(issue))}
              </p>
            ) : progress?.phase === 'ready' ? (
              <p role="status">{t('complete')}</p>
            ) : null}
          </div>
          <div className="office-engine-actions">
            <button
              type="button"
              className="button"
              disabled={busy || action}
              onClick={() => void install()}
            >
              {t('choose')}
            </button>
            {busy ? (
              <button
                type="button"
                className="button"
                disabled={cancelRequested}
                onClick={() => void cancel()}
              >
                {t(cancelRequested ? 'stopping' : 'stop')}
              </button>
            ) : null}
          </div>
        </>
      )}
    </section>
  );
}
