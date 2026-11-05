import { useEffect, useId, useRef, useState } from 'react';
import { X } from 'lucide-react';
import type { Language } from '../../lib/types';
import { defaults } from './api';
import type { Collection, Metadata, Unit, UnitKind, Work, WorkKind, Direction } from './api';
import { failureKey, libraryText } from './messages';
export type Editing =
  | { kind: 'collection'; item?: Collection }
  | { kind: 'work'; item?: Work }
  | { kind: 'unit'; item?: Unit }
  | { kind: 'remove'; target: 'collection' | 'work' | 'unit'; item: Collection | Work | Unit };
export function Editor({
  editing,
  language,
  onSave,
  onClose,
}: {
  editing: Editing;
  language: Language;
  onSave: (values: {
    title: string;
    description: string;
    metadata: Metadata;
    unitKind: UnitKind;
  }) => Promise<void>;
  onClose: () => void;
}) {
  const t = (key: Parameters<typeof libraryText>[1]) => libraryText(language, key);
  const initial = 'item' in editing ? editing.item : undefined;
  const meta = editing.kind === 'work' ? editing.item?.metadata : undefined;
  const [title, setTitle] = useState(initial?.title ?? '');
  const [description, setDescription] = useState(
    editing.kind === 'collection' ? (editing.item?.description ?? '') : (meta?.description ?? ''),
  );
  const [kind, setKind] = useState<WorkKind>(meta?.kind ?? 'document');
  const [direction, setDirection] = useState<Direction>(meta?.direction ?? 'leftToRight');
  const [authors, setAuthors] = useState(meta?.authors.join(', ') ?? '');
  const [tags, setTags] = useState(meta?.tags.join(', ') ?? '');
  const [externalId, setExternalId] = useState(meta?.externalId ?? '');
  const [unitKind, setUnitKind] = useState<UnitKind>(
    editing.kind === 'unit' ? (editing.item?.kind ?? 'chapter') : 'chapter',
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const id = useId();
  useEffect(() => {
    const node = dialog.current;
    node?.showModal();
    return () => node?.close();
  }, []);
  const heading =
    editing.kind === 'remove'
      ? t('remove')
      : initial
        ? t('edit')
        : t(
            editing.kind === 'collection'
              ? 'newCollection'
              : editing.kind === 'work'
                ? 'newWork'
                : 'newUnit',
          );
  return (
    <dialog
      ref={dialog}
      className="modal collection-dialog"
      aria-labelledby={id}
      onKeyDown={(event) => event.stopPropagation()}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          setBusy(true);
          setError(null);
          const list = (s: string) =>
            s
              .split(',')
              .map((x) => x.trim())
              .filter(Boolean);
          void onSave({
            title,
            description,
            unitKind,
            metadata: {
              ...defaults,
              kind,
              direction,
              authors: list(authors),
              tags: list(tags),
              description,
              externalId: externalId.trim() || null,
            },
          })
            .catch(setError)
            .finally(() => setBusy(false));
        }}
      >
        <header>
          <h2 id={id}>{heading}</h2>
          <button
            type="button"
            className="icon-button"
            aria-label={t('close')}
            disabled={busy}
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </header>
        {editing.kind === 'remove' ? (
          <div className="collection-remove-copy">
            <strong>{editing.item.title}</strong>
            <p>{t('removeHint')}</p>
          </div>
        ) : (
          <div className="collection-fields">
            <label>
              {t('name')}
              <input
                autoFocus
                required
                maxLength={512}
                value={title}
                disabled={busy}
                onChange={(event) => setTitle(event.target.value)}
              />
            </label>
            {editing.kind === 'unit' ? (
              <label>
                {t('kind')}
                <select
                  value={unitKind}
                  aria-label={t('kind')}
                  disabled={busy}
                  onChange={(event) => setUnitKind(event.target.value as UnitKind)}
                >
                  {(['volume', 'chapter', 'part', 'document'] as const).map((value) => (
                    <option value={value} key={value}>
                      {t(value)}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            {editing.kind === 'work' ? (
              <>
                <div className="collection-field-pair">
                  <label>
                    {t('kind')}
                    <select
                      value={kind}
                      aria-label={t('kind')}
                      disabled={busy}
                      onChange={(event) => {
                        const value = event.target.value as WorkKind;
                        setKind(value);
                        if (!meta && direction === 'leftToRight')
                          setDirection(
                            value === 'webtoon'
                              ? 'vertical'
                              : value === 'comic' || value === 'hitomi'
                                ? 'rightToLeft'
                                : 'leftToRight',
                          );
                      }}
                    >
                      {(
                        [
                          'document',
                          'image',
                          'comic',
                          'webtoon',
                          'novel',
                          'hitomi',
                          'other',
                        ] as const
                      ).map((value) => (
                        <option value={value} key={value}>
                          {t(value)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label>
                    {t('direction')}
                    <select
                      value={direction}
                      aria-label={t('direction')}
                      disabled={busy}
                      onChange={(event) => setDirection(event.target.value as Direction)}
                    >
                      {(['leftToRight', 'rightToLeft', 'vertical'] as const).map((value) => (
                        <option value={value} key={value}>
                          {t(value)}
                        </option>
                      ))}
                    </select>
                  </label>
                </div>
                <label>
                  {t('authors')}
                  <input
                    value={authors}
                    disabled={busy}
                    onChange={(event) => setAuthors(event.target.value)}
                  />
                </label>
                <label>
                  {t('tags')}
                  <input
                    value={tags}
                    disabled={busy}
                    onChange={(event) => setTags(event.target.value)}
                  />
                </label>
                <label>
                  {t('externalId')}
                  <input
                    value={externalId}
                    maxLength={256}
                    disabled={busy}
                    onChange={(event) => setExternalId(event.target.value)}
                  />
                </label>
              </>
            ) : null}
            {editing.kind !== 'unit' ? (
              <label>
                {t('description')}
                <textarea
                  rows={3}
                  maxLength={16384}
                  value={description}
                  disabled={busy}
                  onChange={(event) => setDescription(event.target.value)}
                />
              </label>
            ) : null}
          </div>
        )}
        {error ? (
          <p role="alert" className="collection-error">
            {t(failureKey(error))}
          </p>
        ) : null}
        <footer>
          <button type="button" className="button" disabled={busy} onClick={onClose}>
            {t('cancel')}
          </button>
          <button
            className={`button ${editing.kind === 'remove' ? 'collection-danger' : 'primary'}`}
            disabled={busy || (editing.kind !== 'remove' && !title.trim())}
          >
            {busy ? t('loading') : editing.kind === 'remove' ? t('remove') : t('save')}
          </button>
        </footer>
      </form>
    </dialog>
  );
}
