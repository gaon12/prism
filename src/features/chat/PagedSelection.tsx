import { useEffect, useRef, useState } from 'react';
import { rag, type SelectionPage } from '../../lib/rag';
import type { Language } from '../../lib/types';
import { chatText } from './messages';

/** Only one page and its edits live in the webview, even for a large snapshot. */
export function PagedSelection({
  id,
  initialPage,
  language,
  disabled,
  onSaved,
  onDirty,
}: {
  id: string;
  initialPage: SelectionPage;
  language: Language;
  disabled: boolean;
  onSaved: (page: SelectionPage | null) => void;
  onDirty: (dirty: boolean) => void;
}) {
  const t = (key: Parameters<typeof chatText>[1]) => chatText(language, key);
  const [page, setPage] = useState(initialPage);
  const [changes, setChanges] = useState<Map<number, boolean>>(new Map());
  const [bulk, setBulk] = useState<boolean | null>(null);
  const [working, setWorking] = useState(false);
  const [failed, setFailed] = useState(false);
  const alive = useRef(false);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (list.current) list.current.scrollTop = 0;
  }, [page.offset]);
  const serialized = JSON.stringify(initialPage);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    setPage(JSON.parse(serialized) as SelectionPage);
    setChanges(new Map());
    setBulk(null);
    setFailed(false);
  }, [serialized]);
  const dirty = bulk !== null || changes.size > 0;
  useEffect(() => {
    onDirty(dirty || working);
    return () => onDirty(false);
  }, [dirty, working, onDirty]);
  const chosen = (file: SelectionPage['files'][number]) =>
    changes.get(file.id) ?? bulk ?? file.selected;
  const selectedCount =
    (bulk === null ? page.selectedCount : bulk ? page.total : 0) +
    page.files.reduce(
      (count, file) => count + Number(chosen(file)) - Number(bulk ?? file.selected),
      0,
    );
  const run = async (work: () => Promise<SelectionPage | null>) => {
    if (working || disabled) return;
    setWorking(true);
    setFailed(false);
    try {
      const next = await work();
      if (alive.current) {
        if (next) {
          setPage(next);
          setChanges(new Map());
          setBulk(null);
        }
        onSaved(next);
      }
    } catch {
      if (alive.current) setFailed(true);
    } finally {
      if (alive.current) setWorking(false);
    }
  };
  const load = (offset: number) => run(() => rag.fileSetPage(id, offset));
  const save = () =>
    run(async () => {
      await rag.editFileSet(id, page.revision, [...changes], bulk);
      return rag.fileSetPage(id, page.offset);
    });
  const clear = () =>
    run(async () => {
      await rag.editFileSet(id, page.revision, [], null, true);
      return null;
    });
  return (
    <details className="chat-search-selection" data-paged="true">
      <summary>
        {chatText(language, 'searchSelectionCount', { count: selectedCount, total: page.total })}
        {page.partial ? ` · ${t('partialSelection')}` : ''}
      </summary>
      <p className="chat-note">{t('searchSelectionHelp')}</p>
      {page.query ? <p className="chat-selection-query">{page.query}</p> : null}
      {page.partial ? <p className="chat-scope-warning">{t('snapshotIncomplete')}</p> : null}
      <fieldset disabled={disabled || working}>
        <legend>{t('searchSelectionFiles')}</legend>
        <div className="chat-selection-actions">
          <button
            type="button"
            className="text-button"
            onClick={() => {
              setBulk(true);
              setChanges(new Map());
            }}
          >
            {t('selectAllFiles')}
          </button>
          <button
            type="button"
            className="text-button"
            onClick={() => {
              setBulk(false);
              setChanges(new Map());
            }}
          >
            {t('clearFiles')}
          </button>
        </div>
        <div className="chat-selection-list" ref={list}>
          {page.files.map((file) => (
            <label className="chat-check" key={file.id}>
              <input
                type="checkbox"
                checked={chosen(file)}
                onChange={(event) => {
                  const selected = event.target.checked;
                  setChanges((current) => {
                    const next = new Map(current);
                    if (selected === (bulk ?? file.selected)) next.delete(file.id);
                    else next.set(file.id, selected);
                    return next;
                  });
                }}
              />
              <span>
                {file.path.split(/[\\/]/u).pop()}
                <small>{file.path}</small>
              </span>
            </label>
          ))}
        </div>
        <div className="chat-selection-actions" role="group" aria-label={t('filePages')}>
          <button
            type="button"
            className="button"
            disabled={dirty || page.offset === 0}
            onClick={() => void load(Math.max(0, page.offset - 50))}
          >
            {t('previousFiles')}
          </button>
          <span className="chat-note">
            {chatText(language, 'filePageRange', {
              start: page.total ? page.offset + 1 : 0,
              end: page.offset + page.files.length,
              total: page.total,
            })}
          </span>
          <button
            type="button"
            className="button"
            disabled={dirty || !page.hasMore}
            onClick={() => void load(page.offset + 50)}
          >
            {t('nextFiles')}
          </button>
        </div>
        {!selectedCount ? <p className="chat-scope-warning">{t('noSelectedFiles')}</p> : null}
        <div className="chat-selection-actions">
          <button type="button" className="button" disabled={!dirty} onClick={() => void save()}>
            {t('saveFileSelection')}
          </button>
          <button type="button" className="text-button" onClick={() => void clear()}>
            {t('useFolderScope')}
          </button>
        </div>
        {dirty ? <p className="chat-note">{t('saveSelectionFirst')}</p> : null}
        {failed ? (
          <div role="alert">
            <p>{t('selectionRefreshFailed')}</p>
            <button type="button" className="text-button" onClick={() => void load(page.offset)}>
              {t('refresh')}
            </button>
          </div>
        ) : null}
      </fieldset>
      {working ? (
        <p role="status" className="chat-note">
          {t('loading')}
        </p>
      ) : null}
    </details>
  );
}
