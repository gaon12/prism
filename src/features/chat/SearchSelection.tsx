import { useEffect, useState } from 'react';
import type { ChatSelection } from '../../lib/rag';
import type { Language } from '../../lib/types';
import { chatText } from './messages';

export function SearchSelection({
  selection,
  language,
  disabled,
  onSave,
  onDirty,
}: {
  selection: ChatSelection;
  language: Language;
  disabled: boolean;
  onSave: (selection: ChatSelection | null) => Promise<void>;
  onDirty: (dirty: boolean) => void;
}) {
  const t = (key: Parameters<typeof chatText>[1]) => chatText(language, key);
  const [chosen, setChosen] = useState(selection.selectedDocuments);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const serialized = JSON.stringify(selection);
  useEffect(() => {
    setChosen((JSON.parse(serialized) as ChatSelection).selectedDocuments);
    setFailed(false);
  }, [serialized]);
  const dirty = JSON.stringify(chosen) !== JSON.stringify(selection.selectedDocuments);
  useEffect(() => {
    onDirty(dirty);
    return () => onDirty(false);
  }, [dirty, onDirty]);
  const save = async (value: ChatSelection | null) => {
    if (saving || disabled) return;
    setSaving(true);
    setFailed(false);
    try {
      await onSave(value);
    } catch {
      setFailed(true);
    } finally {
      setSaving(false);
    }
  };
  return (
    <details className="chat-search-selection">
      <summary>
        {chatText(language, 'searchSelectionCount', {
          count: chosen.length,
          total: selection.documents.length,
        })}
        {selection.partial ? ` · ${t('partialSelection')}` : ''}
      </summary>
      <p className="chat-note">{t('searchSelectionHelp')}</p>
      {selection.query ? <p className="chat-selection-query">{selection.query}</p> : null}
      {selection.partial ? (
        <p className="chat-scope-warning">{t('searchSelectionPartial')}</p>
      ) : null}
      <fieldset disabled={disabled || saving}>
        <legend>{t('searchSelectionFiles')}</legend>
        <div className="chat-selection-actions">
          <button
            type="button"
            className="text-button"
            onClick={() => setChosen([...selection.documents])}
          >
            {t('selectAllFiles')}
          </button>
          <button type="button" className="text-button" onClick={() => setChosen([])}>
            {t('clearFiles')}
          </button>
        </div>
        <div className="chat-selection-list">
          {selection.documents.map((path) => (
            <label className="chat-check" key={path}>
              <input
                type="checkbox"
                checked={chosen.includes(path)}
                onChange={(event) =>
                  setChosen((current) =>
                    event.target.checked
                      ? selection.documents.filter(
                          (value) => value === path || current.includes(value),
                        )
                      : current.filter((value) => value !== path),
                  )
                }
              />
              <span>
                {path.split(/[\\/]/u).pop()}
                <small>{path}</small>
              </span>
            </label>
          ))}
        </div>
        {!chosen.length ? <p className="chat-scope-warning">{t('noSelectedFiles')}</p> : null}
        <div className="chat-selection-actions">
          <button
            type="button"
            className="button"
            disabled={!dirty}
            onClick={() => void save({ ...selection, selectedDocuments: chosen })}
          >
            {t('saveFileSelection')}
          </button>
          <button type="button" className="text-button" onClick={() => void save(null)}>
            {t('useFolderScope')}
          </button>
        </div>
        {dirty ? <p className="chat-note">{t('saveSelectionFirst')}</p> : null}
        {failed ? (
          <p role="alert" className="chat-failure">
            {t('selectionSaveFailed')}
          </p>
        ) : null}
      </fieldset>
    </details>
  );
}
