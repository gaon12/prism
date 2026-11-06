import { useEffect, useRef, useState } from 'react';
import { bridge } from '../../lib/bridge';
import type { Language } from '../../lib/types';
import { resetText } from './messages';
import './index-reset.css';
type Target = 'filename' | 'content';
export function IndexResetPanel({
  language,
  native,
  revision,
  settingsDirty,
  restartBlocked,
}: {
  language: Language;
  native: boolean;
  revision: number;
  settingsDirty: boolean;
  restartBlocked: boolean;
}) {
  const t = (key: Parameters<typeof resetText>[1]) => resetText(language, key);
  const [status, setStatus] = useState<Awaited<ReturnType<typeof bridge.indexResetStatus>> | null>(
    null,
  );
  const [confirm, setConfirm] = useState<Target | null>(null);
  const [started, setStarted] = useState(false);
  useEffect(() => {
    if (!native) return;
    let alive = true;
    void bridge
      .indexResetStatus()
      .then((value) => {
        if (alive) setStatus(value);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [native]);
  if (!native) return null;
  return (
    <section className="index-reset-panel" aria-label={t('title')}>
      <h3>{t('title')}</h3>
      <p className="setting-description">{t('description')}</p>
      <div className="index-reset-actions">
        <button
          type="button"
          className="button compact"
          disabled={settingsDirty}
          onClick={() => setConfirm('filename')}
        >
          {t('filename')}
        </button>
        <button
          type="button"
          className="button compact"
          disabled={settingsDirty || restartBlocked}
          onClick={() => setConfirm('content')}
        >
          {t('content')}
        </button>
      </div>
      {settingsDirty || restartBlocked ? <p className="setting-hint">{t('unsaved')}</p> : null}
      {status?.pending ? (
        <p role="status" className="setting-hint">
          {t('pending')}
        </p>
      ) : null}
      {started || status?.lastApplied ? (
        <p role="status" className="setting-hint">
          {t(started ? 'started' : 'applied')}
        </p>
      ) : null}
      {status?.error ? (
        <p role="alert" className="inline-error">
          {t('failed')}
        </p>
      ) : null}
      {confirm ? (
        <ResetDialog
          target={confirm}
          language={language}
          revision={revision}
          blocked={settingsDirty || (confirm === 'content' && restartBlocked)}
          onClose={() => setConfirm(null)}
          onDone={() => {
            setStarted(true);
            setConfirm(null);
          }}
        />
      ) : null}
    </section>
  );
}
function ResetDialog({
  target,
  language,
  revision,
  blocked,
  onClose,
  onDone,
}: {
  target: Target;
  language: Language;
  revision: number;
  blocked: boolean;
  onClose: () => void;
  onDone: () => void;
}) {
  const t = (key: Parameters<typeof resetText>[1]) => resetText(language, key);
  const dialog = useRef<HTMLDialogElement>(null);
  const running = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const reset = async () => {
    if (running.current || blocked) return;
    running.current = true;
    setBusy(true);
    setError(false);
    try {
      await bridge.resetIndex(target, revision);
      onDone();
    } catch {
      setError(true);
    } finally {
      running.current = false;
      setBusy(false);
    }
  };
  return (
    <dialog
      ref={dialog}
      className="index-reset-dialog"
      aria-label={t(target)}
      onCancel={(event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!busy) onClose();
      }}
    >
      <h2>{t(target)}</h2>
      <p>{t(target === 'filename' ? 'filenameBody' : 'contentBody')}</p>
      {target === 'content' ? <p>{t('ocr')}</p> : null}
      {blocked ? <p role="alert">{t('unsaved')}</p> : null}
      {error ? (
        <p role="alert" className="inline-error">
          {t('failed')}
        </p>
      ) : null}
      {busy ? <p role="status">{t('working')}</p> : null}
      <div className="index-reset-actions">
        <button type="button" className="button" disabled={busy} onClick={onClose}>
          {t('cancel')}
        </button>
        <button
          type="button"
          className="button primary"
          disabled={busy || blocked}
          onClick={() => void reset()}
        >
          {t(target === 'content' ? 'restart' : 'collect')}
        </button>
      </div>
    </dialog>
  );
}
