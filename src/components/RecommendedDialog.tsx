import { useEffect, useRef, useState } from 'react';
import { Folder, X } from 'lucide-react';
import { useClient, saveSettings, scopesFromPaths, showToast, errorMessage } from '../lib/store';
import { translate } from '../lib/i18n';
import type { RecommendedFolder } from '../lib/types';
export function RecommendedDialog({
  folders,
  onClose,
}: {
  folders: RecommendedFolder[];
  onClose: () => void;
}) {
  const { settings } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const dialog = useRef<HTMLDialogElement>(null);
  const [paths, setPaths] = useState(() => new Set(folders.map((folder) => folder.path)));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const submit = async () => {
    setBusy(true);
    try {
      await saveSettings({ ...settings, scopes: scopesFromPaths([...paths], settings.scopes) });
      showToast(t('folderAdded'));
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <dialog
      ref={dialog}
      className="modal recommendation-modal"
      aria-labelledby="recommend-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <div className="modal-header">
        <h2 id="recommend-title">{t('recommended')}</h2>
        <button className="icon-button" aria-label={t('close')} onClick={onClose}>
          <X size={18} />
        </button>
      </div>
      <div className="recommend-body">
        <p className="setting-description">{t('scopeDescription')}</p>
        {folders.map((folder) => (
          <label className="recommended-folder" key={folder.path}>
            <input
              type="checkbox"
              checked={paths.has(folder.path)}
              onChange={(event) =>
                setPaths((previous) => {
                  const next = new Set(previous);
                  if (event.target.checked) next.add(folder.path);
                  else next.delete(folder.path);
                  return next;
                })
              }
            />
            <Folder size={20} />
            <span>
              <strong>{folder.name}</strong>
              <small>{folder.path}</small>
            </span>
          </label>
        ))}
        {error ? (
          <p role="alert" className="form-error">
            {error}
          </p>
        ) : null}
      </div>
      <div className="modal-footer">
        <button className="button" onClick={onClose}>
          {t('cancel')}
        </button>
        <button
          className="button primary"
          disabled={!paths.size || busy}
          onClick={() => void submit()}
        >
          {t('addSelected')}
        </button>
      </div>
    </dialog>
  );
}
