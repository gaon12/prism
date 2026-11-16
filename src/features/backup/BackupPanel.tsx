// Settings → General: make a backup folder, or restore one (applied after a restart).
import { useEffect, useRef, useState } from 'react';
import { bridge } from '../../lib/bridge';
import { errorMessage, showToast } from '../../lib/store';
import type { Language } from '../../lib/types';
import { backupFailure, backupText, type BackupMessage } from './messages';
import '../index-reset/index-reset.css';

export function BackupPanel({ language, native }: { language: Language; native: boolean }) {
  const t = (key: BackupMessage, values?: Record<string, string>) =>
    backupText(language, key, values);
  const [status, setStatus] = useState<Awaited<ReturnType<typeof bridge.backupStatus>> | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The folder chosen for a restore, waiting for confirmation.
  const [restoreFrom, setRestoreFrom] = useState<string | null>(null);
  const refresh = () =>
    bridge
      .backupStatus()
      .then(setStatus)
      .catch(() => {});
  useEffect(() => {
    if (native) void refresh();
  }, [native]);
  if (!native) return null;
  // Runs one action, showing a localized message if it fails.
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(backupFailure(language, errorMessage(cause)));
    } finally {
      setBusy(false);
    }
  };
  const create = () =>
    run(async () => {
      const [folder] = await bridge.chooseFolders();
      if (!folder) return;
      const path = await bridge.createBackup(folder);
      showToast(t('created', { path }));
    });
  const chooseRestore = () =>
    run(async () => {
      const [folder] = await bridge.chooseFolders();
      if (folder) setRestoreFrom(folder);
    });
  return (
    <section className="index-reset-panel" aria-label={t('title')}>
      <h3>{t('title')}</h3>
      <p className="setting-description">{t('description')}</p>
      <div className="index-reset-actions">
        <button
          type="button"
          className="button compact"
          disabled={busy}
          onClick={() => void create()}
        >
          {t('create')}
        </button>
        <button
          type="button"
          className="button compact"
          disabled={busy}
          onClick={() => void chooseRestore()}
        >
          {t('restore')}
        </button>
        {status?.pending ? (
          <button
            type="button"
            className="button compact"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await bridge.restoreBackup(null);
                await refresh();
              })
            }
          >
            {t('cancelPending')}
          </button>
        ) : null}
      </div>
      {status?.pending ? (
        <p role="status" className="setting-hint">
          {t('pending')}
        </p>
      ) : null}
      {status?.restored ? (
        <p role="status" className="setting-hint">
          {t('restored')}
        </p>
      ) : null}
      {status?.error ? (
        <p role="alert" className="inline-error">
          {t('restoreFailed')}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="inline-error">
          {error}
        </p>
      ) : null}
      {restoreFrom ? (
        <RestoreDialog
          folder={restoreFrom}
          language={language}
          onClose={() => setRestoreFrom(null)}
        />
      ) : null}
    </section>
  );
}

function RestoreDialog({
  folder,
  language,
  onClose,
}: {
  folder: string;
  language: Language;
  onClose: () => void;
}) {
  const t = (key: BackupMessage) => backupText(language, key);
  const dialog = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const restore = async () => {
    setBusy(true);
    setError(null);
    try {
      // On success Prism restarts and this window goes away.
      await bridge.restoreBackup(folder);
    } catch (cause) {
      setError(backupFailure(language, errorMessage(cause)));
      setBusy(false);
    }
  };
  return (
    <dialog
      ref={dialog}
      className="index-reset-dialog"
      aria-label={t('confirmTitle')}
      onCancel={(event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!busy) onClose();
      }}
    >
      <h2>{t('confirmTitle')}</h2>
      <p>{t('confirmBody')}</p>
      <p className="setting-hint" title={folder}>
        {folder}
      </p>
      {error ? (
        <p role="alert" className="inline-error">
          {error}
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
          disabled={busy}
          onClick={() => void restore()}
        >
          {t('confirm')}
        </button>
      </div>
    </dialog>
  );
}
