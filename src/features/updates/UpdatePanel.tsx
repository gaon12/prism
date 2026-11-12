import { useEffect, useRef, useState } from 'react';
import { Download, RefreshCw } from 'lucide-react';
import { native } from '../../lib/bridge';
import { useClient, showToast, errorMessage } from '../../lib/store';
import { translate } from '../../lib/i18n';
import { updates, useUpdates } from '../../lib/updates';
import { updateText } from './messages';
import './updates.css';

function InstallAction({ blocked = false }: { blocked?: boolean }) {
  const { view } = useUpdates();
  const { settings } = useClient();
  const t = (key: Parameters<typeof updateText>[1]) => updateText(settings.language, key);
  const [confirm, setConfirm] = useState<string | null>(null);
  return (
    <>
      <button
        type="button"
        className="button primary"
        disabled={blocked || view?.checking || view?.installing}
        onClick={() => setConfirm(view?.available?.version ?? null)}
      >
        {t('install')}
      </button>
      {blocked ? <small className="muted">{t('unsaved')}</small> : null}
      {confirm ? <InstallDialog version={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}
function InstallDialog({ onClose, version }: { onClose: () => void; version: string }) {
  const { settings } = useClient();
  const t = (key: Parameters<typeof updateText>[1]) => updateText(settings.language, key);
  const dialog = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<'install' | 'changed' | null>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const install = async () => {
    setBusy(true);
    setFailed(null);
    try {
      await updates.install(version);
      onClose();
    } catch (cause) {
      setFailed(errorMessage(cause).includes('UPDATE_CHANGED') ? 'changed' : 'install');
    } finally {
      setBusy(false);
    }
  };
  return (
    <dialog
      ref={dialog}
      className="update-dialog"
      aria-label={t('confirm')}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <h2>{t('confirm')}</h2>
      <p className="update-version">Prism {version}</p>
      <p>{t('confirmBody')}</p>
      {failed ? (
        <p role="alert" className="inline-error">
          {t(failed === 'changed' ? 'changed' : 'installFailed')}
        </p>
      ) : null}
      {busy ? <p role="status">{t('installing')}</p> : null}
      <div className="update-actions">
        <button type="button" className="button" disabled={busy} onClick={onClose}>
          {t('cancel')}
        </button>
        <button
          type="button"
          className="button primary"
          disabled={busy || failed === 'changed'}
          onClick={() => void install()}
        >
          {t('install')}
        </button>
      </div>
    </dialog>
  );
}
function DownloadAction() {
  const { settings } = useClient();
  return (
    <button
      type="button"
      className="button"
      onClick={() =>
        void updates
          .openDownload()
          .catch(() => showToast(updateText(settings.language, 'checkFailed')))
      }
    >
      <Download size={15} />
      {updateText(settings.language, 'download')}
    </button>
  );
}
/** Which package manager owns updates for this copy, if any. */
function managedBy(
  installKind: string | null,
  reason: string | null | undefined,
): 'packaged' | 'flatpak' | null {
  if (installKind === 'packaged' || reason === 'packaged') return 'packaged';
  if (installKind === 'flatpak' || reason === 'flatpak') return 'flatpak';
  return null;
}
export function UpdateBanner({ blocked = false }: { blocked?: boolean }) {
  const { settings, runtime } = useClient();
  const { view } = useUpdates();
  // MSIX and Flatpak copies are updated by their package manager, not by Prism.
  const managed = managedBy(runtime.installKind, view?.installDisabledReason);
  const t = (key: Parameters<typeof updateText>[1]) => updateText(settings.language, key);
  if (!native || blocked || !view?.available || view.dismissedVersion === view.available.version)
    return null;
  return (
    <aside className="update-banner" role="status" aria-label={t('title')}>
      <strong>
        {t('available')} · {view.available.version}
      </strong>
      <div className="update-actions">
        {managed ? (
          <span>{t(managed)}</span>
        ) : view.canInstall ? (
          <InstallAction />
        ) : (
          <DownloadAction />
        )}
        <button
          type="button"
          className="text-button"
          disabled={view.installing}
          onClick={() => void updates.dismiss().catch(() => showToast(t('stateFailed')))}
        >
          {t('later')}
        </button>
      </div>
    </aside>
  );
}
export function UpdatePanel({
  autoCheck,
  onAutoCheck,
  installBlocked = false,
}: {
  autoCheck: boolean;
  onAutoCheck: (value: boolean) => void;
  installBlocked?: boolean;
}) {
  const { settings, runtime } = useClient();
  const { view, error } = useUpdates();
  // MSIX and Flatpak copies are updated by their package manager, not by Prism.
  const managed = managedBy(runtime.installKind, view?.installDisabledReason);
  const t = (key: Parameters<typeof updateText>[1]) => updateText(settings.language, key);
  const [busy, setBusy] = useState(false);
  const [checked, setChecked] = useState<'ok' | 'failed' | null>(null);
  useEffect(() => {
    if (view && !view.checkError)
      setChecked((previous) => (previous === 'failed' ? null : previous));
  }, [view?.lastCheckMs, view?.checkError]);
  const check = async () => {
    setBusy(true);
    setChecked(null);
    try {
      const result = await updates.check();
      setChecked(result.checkError ? 'failed' : 'ok');
    } catch {
      setChecked('failed');
    } finally {
      setBusy(false);
    }
  };
  const date = view?.lastCheckMs ? new Date(view.lastCheckMs) : null;
  return (
    <section className="update-panel" aria-label={t('title')}>
      <h3>
        {t('title')}
        {view ? <small className="muted"> · Prism {view.currentVersion}</small> : null}
      </h3>
      <label className="setting-check">
        <span>{t('auto')}</span>
        <input
          type="checkbox"
          checked={autoCheck}
          onChange={(event) => onAutoCheck(event.target.checked)}
        />
      </label>
      <p className="setting-footnote">{t('schedule')}</p>
      {managed ? <p className="setting-footnote">{t(managed)}</p> : null}
      {!native ? <p className="muted">{translate(settings.language, 'nativeOnly')}</p> : null}
      {error || view?.stateError ? (
        <p className="inline-error" role="alert">
          {t('stateFailed')}
        </p>
      ) : null}
      <p className="muted">
        {date && !Number.isNaN(date.getTime())
          ? `${t('checked')} · ${date.toLocaleString(settings.language)}`
          : t('never')}
      </p>
      {checked === 'failed' ? (
        <p role="alert" className="inline-error">
          {t('checkFailed')}
        </p>
      ) : checked === 'ok' && !view?.available && !view?.checkError ? (
        <p role="status">{t('current')}</p>
      ) : null}
      {view?.available ? (
        <>
          <p>
            <strong>
              {t('available')} · {view.available.version}
            </strong>
          </p>
          <p className="update-notes">{view.available.notes}</p>
          {!view.canInstall && !managed ? <p className="muted">{t('manual')}</p> : null}
        </>
      ) : null}
      <div className="update-actions">
        <button
          type="button"
          className="button"
          disabled={!native || busy || view?.checking || view?.installing}
          onClick={() => void check()}
        >
          <RefreshCw size={15} />
          {busy || view?.checking ? t('checking') : t('check')}
        </button>
        {managed ? null : view?.available ? (
          view.canInstall ? (
            <InstallAction blocked={installBlocked} />
          ) : (
            <DownloadAction />
          )
        ) : native && error ? (
          <DownloadAction />
        ) : null}
      </div>
    </section>
  );
}
