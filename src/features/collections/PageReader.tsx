import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight, X, FileText } from 'lucide-react';
import type { Language } from '../../lib/types';
import { library, readingSettled, mediaTask } from './api';
import type { Unit, Work } from './api';
import { useRemote } from './hooks';
import { failureKey, libraryText } from './messages';
import { EpubPage } from './EpubPage';
import { textDocument } from './text-document';
export function PageReader({
  work,
  unit,
  initialOffset,
  initialCompleted,
  language,
  onClose,
  onRefresh,
}: {
  work: Work;
  unit: Unit;
  initialOffset: number;
  initialCompleted: boolean;
  language: Language;
  onClose: () => void;
  onRefresh: () => void;
}) {
  const t = (key: Parameters<typeof libraryText>[1], values?: Record<string, string | number>) =>
    libraryText(language, key, values);
  const [offset, setOffset] = useState(initialOffset);
  const [completed, setCompleted] = useState(initialCompleted);
  const [fitWidth, setFitWidth] = useState(work.metadata.direction === 'vertical');
  const [writeError, setWriteError] = useState<unknown>(null);
  const ref = useRef<HTMLDialogElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const heading = useId();
  const location = useRemote(`${unit.id}:${unit.revision}:${offset}`, 0, () =>
    library.pages(unit, offset, 1),
  );
  const page = location.data?.items[0];
  const total = location.data?.total ?? unit.pageCount;
  const media = useRemote(page?.id ?? '', 0, (cancelled) =>
    page ? mediaTask(() => library.media(page.id), cancelled, true) : Promise.resolve(null),
  );
  const readerDocument = useMemo(
    () =>
      media.data?.document ??
      (media.data?.text !== null && media.data?.text !== undefined
        ? textDocument(media.data.text)
        : null),
    [media.data],
  );
  useEffect(() => {
    const node = ref.current;
    node?.showModal();
    return () => node?.close();
  }, []);
  useEffect(() => {
    content.current?.scrollTo(0, 0);
  }, [page?.id]);
  useEffect(() => {
    if (!media.data || !page || media.data.pageId !== page.id) return;
    let active = true;
    void library.saveReading(work.id, page.id, completed).catch((error) => {
      if (active) setWriteError(error);
    });
    return () => {
      active = false;
    };
  }, [media.data?.pageId, page?.id, completed, work.id]);
  const close = () => {
    onClose();
    void readingSettled(work.id)
      .finally(onRefresh)
      .catch(() => {});
  };
  const previous = () => setOffset((n) => Math.max(0, n - 1));
  const next = () => setOffset((n) => Math.min(Math.max(0, total - 1), n + 1));
  const error =
    location.error ??
    (location.data && !page ? { code: 'notFound' } : null) ??
    media.error ??
    writeError;
  return (
    <dialog
      ref={ref}
      className="collection-reader"
      aria-labelledby={heading}
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement)
          return;
        const rtl = work.metadata.direction === 'rightToLeft';
        if (event.key === 'ArrowLeft') {
          event.preventDefault();
          rtl ? next() : previous();
        } else if (event.key === 'ArrowRight') {
          event.preventDefault();
          rtl ? previous() : next();
        } else if (event.key === 'PageDown') {
          event.preventDefault();
          next();
        } else if (event.key === 'PageUp') {
          event.preventDefault();
          previous();
        }
      }}
    >
      <header>
        <div>
          <h2 id={heading}>{work.title}</h2>
          <span>{unit.title}</span>
          {media.data?.text !== null && media.data?.text !== undefined ? (
            <span className="collection-text-label">
              <FileText size={14} />
              {t('textPreview')}
            </span>
          ) : null}
        </div>
        <button type="button" className="icon-button" aria-label={t('close')} onClick={close}>
          <X size={20} />
        </button>
      </header>
      <div
        ref={content}
        className={`collection-reader-page ${fitWidth ? 'fit-width' : 'fit-page'}`}
        aria-busy={location.loading || media.loading}
      >
        {error ? (
          <div className="collection-reader-error" role="alert">
            <p>{t(failureKey(error))}</p>
            <button
              className="button"
              onClick={() => {
                onRefresh();
                close();
              }}
            >
              {t('reload')}
            </button>
          </div>
        ) : null}
        {!error && (location.loading || media.loading) ? <p role="status">{t('loading')}</p> : null}
        {!error && media.data?.dataUrl ? (
          <img src={media.data.dataUrl} alt={t('pagePosition', { n: offset + 1, total })} />
        ) : null}
        {!error && readerDocument ? (
          <EpubPage
            key={media.data?.pageId}
            book={readerDocument}
            fitWidth={fitWidth}
            title={page?.title ?? work.title}
            onKey={(key) => {
              if (key === 'Escape') {
                close();
                return;
              }
              const rtl = work.metadata.direction === 'rightToLeft';
              if (key === 'PageDown' || key === (rtl ? 'ArrowLeft' : 'ArrowRight')) next();
              else previous();
            }}
          />
        ) : null}
      </div>
      <footer>
        <div className="collection-reader-nav">
          <button
            type="button"
            className="button"
            disabled={offset === 0}
            onClick={previous}
            aria-label={t('previous')}
          >
            {work.metadata.direction === 'rightToLeft' ? (
              <ChevronRight size={18} />
            ) : (
              <ChevronLeft size={18} />
            )}
          </button>
          <span aria-live="polite">
            {t(
              media.data?.document && !media.data.document.fixed
                ? 'chapterPosition'
                : 'pagePosition',
              { n: Math.min(offset + 1, total), total },
            )}
          </span>
          <button
            type="button"
            className="button"
            disabled={offset + 1 >= total}
            onClick={next}
            aria-label={t('next')}
          >
            {work.metadata.direction === 'rightToLeft' ? (
              <ChevronLeft size={18} />
            ) : (
              <ChevronRight size={18} />
            )}
          </button>
        </div>
        <div>
          {!media.data?.document || media.data.document.fixed ? (
            <button type="button" className="button" onClick={() => setFitWidth((value) => !value)}>
              {t(fitWidth ? 'fitPage' : 'fitWidth')}
            </button>
          ) : null}
          <label>
            <input
              type="checkbox"
              checked={completed}
              onChange={(event) => setCompleted(event.target.checked)}
            />
            {t('completed')}
          </label>
        </div>
      </footer>
    </dialog>
  );
}
