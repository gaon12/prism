import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import {
  BookOpen,
  ChevronLeft,
  ChevronRight,
  FolderPlus,
  ImagePlus,
  Plus,
  RefreshCw,
  Settings2,
  Trash2,
  ArrowUp,
  ArrowDown,
  Square,
  FileText,
  FolderOpen,
} from 'lucide-react';
import { native } from '../../lib/bridge';
import { useClient } from '../../lib/store';
import { library } from './api';
import type { Collection, Unit, Progress } from './api';
import { failureKey, libraryText } from './messages';
import { useRemote } from './hooks';
import { Editor } from './Editor';
import type { Editing } from './Editor';
import { PageReader } from './PageReader';
import { Thumbnail } from './Thumbnail';
import './collections.css';

export function CollectionsView() {
  const { settings } = useClient();
  const language = settings.language;
  const t = (key: Parameters<typeof libraryText>[1], values?: Record<string, string | number>) =>
    libraryText(language, key, values);
  const [revision, setRevision] = useState(0);
  const reload = useCallback(() => setRevision((n) => n + 1), []);
  const [collectionOffset, setCollectionOffset] = useState(0);
  const [workOffset, setWorkOffset] = useState(0);
  const [collectionId, setCollectionId] = useState('');
  const [workId, setWorkId] = useState('');
  const [removed, setRemoved] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState<Editing | null>(null);
  const collections = useRemote('collections:' + collectionOffset, revision, () =>
    native
      ? library.list(collectionOffset)
      : Promise.resolve({ items: [], total: 0, nextOffset: null }),
  );
  const selected = useRemote(collectionId, revision, () =>
    collectionId ? library.get(collectionId) : Promise.resolve(null),
  );
  const works = useRemote(collectionId + ':' + workOffset, revision, () =>
    collectionId
      ? library.works(collectionId, workOffset)
      : Promise.resolve({ items: [], total: 0, nextOffset: null }),
  );
  useEffect(() => {
    const first = collections.data?.items.find((item) => !removed.has(item.id));
    if (!collectionId && first) setCollectionId(first.id);
  }, [collectionId, collections.data, removed]);
  useEffect(() => {
    setWorkOffset(0);
    setWorkId('');
  }, [collectionId]);
  if (!native)
    return (
      <section className="collection-workspace">
        <h1>{t('title')}</h1>
        <p>{t('nativeOnly')}</p>
      </section>
    );
  const collection = selected.data;
  const choices = (collections.data?.items ?? []).filter((item) => !removed.has(item.id));
  if (collection && !choices.some((item) => item.id === collection.id)) choices.push(collection);
  return (
    <section className="collection-workspace" aria-label={t('title')}>
      <header className="collection-heading">
        <div>
          <h1>{t('title')}</h1>
          <p>{t('intro')}</p>
        </div>
        <button type="button" className="button" onClick={() => setEditing({ kind: 'collection' })}>
          <Plus size={16} />
          {t('newCollection')}
        </button>
      </header>
      <div className="collection-toolbar">
        <select
          aria-label={t('chooseCollection')}
          value={collectionId}
          onChange={(event) => setCollectionId(event.target.value)}
        >
          <option value="" disabled>
            {t('chooseCollection')}
          </option>
          {choices.map((item) => (
            <option key={item.id} value={item.id}>
              {item.title}
            </option>
          ))}
        </select>
        {(collections.data?.total ?? 0) > 100 ? (
          <>
            <button
              className="icon-button"
              aria-label={t('previous')}
              disabled={collectionOffset === 0}
              onClick={() => setCollectionOffset((n) => Math.max(0, n - 100))}
            >
              <ChevronLeft size={16} />
            </button>
            <button
              className="icon-button"
              aria-label={t('next')}
              disabled={collections.data?.nextOffset == null}
              onClick={() => setCollectionOffset(collections.data?.nextOffset ?? 0)}
            >
              <ChevronRight size={16} />
            </button>
          </>
        ) : null}
        <button type="button" className="icon-button" aria-label={t('reload')} onClick={reload}>
          <RefreshCw size={16} />
        </button>
        {collection ? (
          <>
            <button
              className="icon-button"
              aria-label={t('edit')}
              onClick={() => setEditing({ kind: 'collection', item: collection })}
            >
              <Settings2 size={16} />
            </button>
            <button
              className="icon-button"
              aria-label={t('remove')}
              onClick={() => setEditing({ kind: 'remove', target: 'collection', item: collection })}
            >
              <Trash2 size={16} />
            </button>
          </>
        ) : null}
        {workId ? (
          <button className="button collection-back" onClick={() => setWorkId('')}>
            <ChevronLeft size={16} />
            {collection?.title}
          </button>
        ) : (
          <button
            className="button primary collection-new-work"
            disabled={!collection || selected.loading}
            onClick={() => setEditing({ kind: 'work' })}
          >
            <Plus size={16} />
            {t('newWork')}
          </button>
        )}
      </div>
      {collections.error || selected.error ? (
        <p className="collection-error" role="alert">
          {t(failureKey(collections.error ?? selected.error))}
        </p>
      ) : null}
      {workId ? (
        <WorkDetail
          key={workId}
          id={workId}
          revision={revision}
          reload={reload}
          collections={choices}
          onMove={(id) => setCollectionId(id)}
          onBack={() => setWorkId('')}
        />
      ) : (
        <>
          {works.error ? (
            <p className="collection-error" role="alert">
              {t(failureKey(works.error))}
            </p>
          ) : null}
          {!collection && !collections.loading ? (
            <div className="collection-empty">
              <BookOpen size={38} />
              <h2>{t('empty')}</h2>
              <p>{t('references')}</p>
              <button className="button primary" onClick={() => setEditing({ kind: 'collection' })}>
                {t('newCollection')}
              </button>
            </div>
          ) : null}
          {collection && works.loading && !works.data ? <p role="status">{t('loading')}</p> : null}
          {collection && works.data?.total === 0 ? (
            <div className="collection-empty">
              <BookOpen size={38} />
              <h2>{collection.title}</h2>
              <p>{t('emptyWork')}</p>
              <button className="button primary" onClick={() => setEditing({ kind: 'work' })}>
                {t('newWork')}
              </button>
            </div>
          ) : null}
          <div className="collection-work-grid" aria-busy={works.loading}>
            {works.data?.items.map((work) => (
              <button
                type="button"
                className="collection-work-card"
                disabled={works.loading}
                key={work.id}
                onClick={() => setWorkId(work.id)}
                aria-label={work.title}
              >
                <Thumbnail kind="work" id={work.id} revision={revision} language={language} />
                <strong>{work.title}</strong>
                <span>
                  {t(work.metadata.kind)}
                  <i /> {t('units', { n: work.unitCount })}
                </span>
                <small>{t('pages', { n: work.pageCount })}</small>
              </button>
            ))}
          </div>
          {works.data && works.data.total > 24 ? (
            <div className="collection-pagination">
              <button
                className="button"
                disabled={workOffset === 0 || works.loading}
                onClick={() => setWorkOffset((n) => Math.max(0, n - 24))}
              >
                {t('previous')}
              </button>
              <span>
                {Math.floor(workOffset / 24) + 1} / {Math.ceil(works.data.total / 24)}
              </span>
              <button
                className="button"
                disabled={works.data.nextOffset == null || works.loading}
                onClick={() => setWorkOffset(works.data?.nextOffset ?? 0)}
              >
                {t('next')}
              </button>
            </div>
          ) : null}
        </>
      )}
      <p className="collection-reference-note">{t('references')}</p>
      {editing ? (
        <Editor
          key={editing.kind + ('item' in editing ? editing.item?.id : '')}
          editing={editing}
          language={language}
          onClose={() => setEditing(null)}
          onSave={async (values) => {
            if (editing.kind === 'remove') {
              await library.remove(editing.target, editing.item);
              if (editing.target === 'collection') {
                setRemoved((previous) => new Set([...previous, editing.item.id]));
                setCollectionId('');
                setWorkId('');
              }
            } else if (editing.kind === 'collection') {
              const result = editing.item
                ? await library.update(editing.item, values.title, values.description)
                : await library.create(values.title, values.description);
              setCollectionId(result.id);
            } else if (editing.kind === 'work') {
              if (!collection) throw { code: 'notFound' };
              const work = await library.createWork(collection.id, values.title, values.metadata);
              setWorkId(work.id);
            }
            setEditing(null);
            reload();
          }}
        />
      ) : null}
    </section>
  );
}

function WorkDetail({
  id,
  revision,
  reload,
  collections,
  onMove,
  onBack,
}: {
  id: string;
  revision: number;
  reload: () => void;
  collections: Collection[];
  onMove: (id: string) => void;
  onBack: () => void;
}) {
  const { settings } = useClient();
  const language = settings.language;
  const t = (key: Parameters<typeof libraryText>[1], values?: Record<string, string | number>) =>
    libraryText(language, key, values);
  const work = useRemote(id, revision, () => library.work(id));
  const [unitOffset, setUnitOffset] = useState(0);
  const units = useRemote(id + ':units:' + unitOffset, revision, () =>
    library.units(id, unitOffset),
  );
  const reading = useRemote(id + ':reading', revision, () => library.reading(id));
  const [unitId, setUnitId] = useState('');
  const [removedUnits, setRemovedUnits] = useState<Set<string>>(() => new Set());
  const unit = useRemote(unitId, revision, () =>
    unitId ? library.unit(unitId) : Promise.resolve(null),
  );
  const [offset, setOffset] = useState(0);
  const pages = useRemote(`${unitId}:${unit.data?.revision ?? 0}:${offset}`, revision, () =>
    unit.data && !unit.data.prepareError
      ? library.pages(unit.data, offset)
      : Promise.resolve({ items: [], total: 0, nextOffset: null }),
  );
  const sources = useRemote(unitId + ':sources', revision, () =>
    unitId ? library.sources(unitId) : Promise.resolve([]),
  );
  const [editing, setEditing] = useState<Editing | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [acting, setBusy] = useState(false);
  const busy = acting || unit.loading || work.loading;
  const [jobs, setJobs] = useState<Record<string, Progress>>({});
  const [reader, setReader] = useState<{ unit: Unit; offset: number } | null>(null);
  useEffect(() => {
    setUnitId('');
    setOffset(0);
    setUnitOffset(0);
  }, [id]);
  useEffect(() => {
    const first = units.data?.items.find((item) => !removedUnits.has(item.id));
    if (!unitId && first) setUnitId(first.id);
  }, [unitId, units.data, removedUnits]);
  useEffect(() => {
    setOffset(0);
  }, [unitId]);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    let epoch = 0;
    void listen<Progress>('prism:collections', (event) => {
      epoch++;
      const progress = event.payload;
      setJobs((previous) => {
        const next = { ...previous };
        if (progress.phase === 'reading') next[progress.unitId] = progress;
        else delete next[progress.unitId];
        return next;
      });
      if (progress.phase !== 'reading' && progress.phase !== 'prepareFailed') reload();
    })
      .then(async (unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        stop = unlisten;
        const snapshot = epoch;
        const active = await library.jobs();
        if (!disposed && epoch === snapshot)
          setJobs(Object.fromEntries(active.map((job) => [job.unitId, job])));
      })
      .catch(() => {});
    return () => {
      disposed = true;
      stop?.();
    };
  }, [reload]);
  const selected = unit.data;
  const current = work.data;
  const importing = Boolean(selected && jobs[selected.id]);
  const act = async (operation: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await operation();
    } catch (cause) {
      setError(cause);
    } finally {
      setBusy(false);
      reload();
    }
  };
  const importPaths = async (paths: string[]) => {
    if (!selected) return;
    setJobs((previous) => ({
      ...previous,
      [selected.id]: { unitId: selected.id, phase: 'reading', processed: 0, error: null },
    }));
    try {
      await library.import(selected, paths);
    } finally {
      setJobs((previous) => {
        const next = { ...previous };
        delete next[selected.id];
        return next;
      });
    }
  };
  const choose = async (directory: boolean) => {
    const result = await open({ directory, multiple: !directory });
    if (result) await importPaths(Array.isArray(result) ? result : [result]);
  };
  if (work.error)
    return (
      <p role="alert" className="collection-error">
        {t(failureKey(work.error))}
      </p>
    );
  if (!current) return <p role="status">{t('loading')}</p>;
  const unitChoices = (units.data?.items ?? []).filter((item) => !removedUnits.has(item.id));
  const unitIndex = (units.data?.items ?? []).findIndex((item) => item.id === unitId);
  const stepUnit = async (item: Unit, forward: boolean) => {
    if (unitIndex < 0) throw { code: 'conflict' };
    const target = await library.units(id, unitOffset + unitIndex + (forward ? 2 : -1), 1);
    return library.moveUnit(item, target.items[0]?.id ?? null);
  };
  if (selected && !unitChoices.some((item) => item.id === selected.id)) unitChoices.push(selected);
  const issue =
    error ??
    units.error ??
    unit.error ??
    pages.error ??
    sources.error ??
    (importing ? null : selected?.prepareError);
  const openReader = (selectedUnit: Unit, index: number) =>
    setReader({ unit: selectedUnit, offset: index });
  return (
    <div className="collection-work-detail">
      <header className="collection-detail-heading">
        <div>
          <h2>{current.title}</h2>
          <p>
            {t(current.metadata.kind)}
            {current.metadata.authors.length ? ` · ${current.metadata.authors.join(', ')}` : ''}
          </p>
          {current.metadata.tags.length ? (
            <div className="collection-tags">
              {current.metadata.tags.map((tag, i) => (
                <span key={i}>{tag}</span>
              ))}
            </div>
          ) : null}
        </div>
        <div className="collection-detail-actions">
          <button
            className="icon-button"
            aria-label={t('edit')}
            onClick={() => setEditing({ kind: 'work', item: current })}
          >
            <Settings2 size={17} />
          </button>
          <button
            className="icon-button"
            aria-label={t('remove')}
            onClick={() => setEditing({ kind: 'remove', target: 'work', item: current })}
          >
            <Trash2 size={17} />
          </button>
          {reading.data?.pageId ? (
            <button
              className="button"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  const address = await library.address(current.id, reading.data!.pageId!);
                  setUnitId(address.unit.id);
                  openReader(address.unit, address.offset);
                })
              }
            >
              <BookOpen size={16} />
              {t('resume')}
            </button>
          ) : null}
        </div>
      </header>
      {current.metadata.description ? (
        <p className="collection-description">{current.metadata.description}</p>
      ) : null}
      <div className="collection-unit-bar">
        <select
          aria-label={t('newUnit')}
          value={unitId}
          onChange={(event) => setUnitId(event.target.value)}
        >
          <option value="" disabled>
            {t('emptyUnit')}
          </option>
          {unitChoices.map((item) => (
            <option key={item.id} value={item.id}>
              {item.title} · {t('pages', { n: item.pageCount })}
            </option>
          ))}
        </select>
        <button className="button" onClick={() => setEditing({ kind: 'unit' })}>
          <Plus size={16} />
          {t('newUnit')}
        </button>
        {selected ? (
          <>
            <button
              className="icon-button"
              aria-label={t('edit')}
              onClick={() => setEditing({ kind: 'unit', item: selected })}
            >
              <Settings2 size={16} />
            </button>
            <button
              className="icon-button"
              aria-label={t('remove')}
              onClick={() => setEditing({ kind: 'remove', target: 'unit', item: selected })}
            >
              <Trash2 size={16} />
            </button>
          </>
        ) : null}
      </div>
      {units.data && units.data.total > 100 ? (
        <div className="collection-pagination">
          <button
            className="button"
            disabled={unitOffset === 0 || units.loading}
            onClick={() => setUnitOffset((n) => Math.max(0, n - 100))}
          >
            {t('previousUnits')}
          </button>
          <span>
            {unitOffset + 1}–{Math.min(unitOffset + 100, units.data.total)} / {units.data.total}
          </span>
          <button
            className="button"
            disabled={units.data.nextOffset == null || units.loading}
            onClick={() => setUnitOffset(units.data?.nextOffset ?? 0)}
          >
            {t('nextUnits')}
          </button>
        </div>
      ) : null}
      {selected ? (
        <div className="collection-import-bar">
          <button
            className="button"
            disabled={busy || importing}
            onClick={() => void act(() => choose(false))}
          >
            <ImagePlus size={16} />
            {t('addFiles')}
          </button>
          <button
            className="button"
            disabled={busy || importing}
            onClick={() => void act(() => choose(true))}
          >
            <FolderPlus size={16} />
            {t('addFolder')}
          </button>
          <button
            className="button"
            disabled={busy || importing || !sources.data?.length}
            onClick={() => void act(() => importPaths([]))}
          >
            <RefreshCw size={16} />
            {t('refresh')}
          </button>
          {selected.pageCount > 0 ? (
            <button
              className="button"
              disabled={busy || importing || Boolean(selected.prepareError)}
              onClick={() => openReader(selected, 0)}
            >
              <BookOpen size={16} />
              {t('reading')}
            </button>
          ) : null}
          <div className="collection-unit-order">
            <button
              className="icon-button"
              aria-label={t('moveUp')}
              disabled={busy || importing || unitIndex < 0 || unitOffset + unitIndex === 0}
              onClick={() => void act(() => stepUnit(selected, false))}
            >
              <ArrowUp size={15} />
            </button>
            <button
              className="icon-button"
              aria-label={t('moveDown')}
              disabled={
                busy ||
                importing ||
                unitIndex < 0 ||
                unitOffset + unitIndex + 1 >= (units.data?.total ?? 0)
              }
              onClick={() => void act(() => stepUnit(selected, true))}
            >
              <ArrowDown size={15} />
            </button>
          </div>
        </div>
      ) : null}
      {unit.loading && jobs[unitId] ? (
        <div className="collection-import-progress" role="status">
          <div
            role="progressbar"
            aria-label={t('updatingPreview')}
            className="collection-progress-track"
          />
          <span>{t('updatingPreview')}</span>
          <button
            className="text-button"
            onClick={() => void library.cancel(unitId).catch(setError)}
          >
            <Square size={12} />
            {t('stop')}
          </button>
        </div>
      ) : null}
      {importing && selected ? (
        <div className="collection-import-progress" role="status">
          <div role="progressbar" aria-label={t('loading')} className="collection-progress-track" />
          <span>{t('importing', { n: jobs[selected.id]?.processed ?? 0 })}</span>
          <button
            className="text-button"
            onClick={() => void library.cancel(selected.id).catch(setError)}
          >
            <Square size={12} />
            {t('stop')}
          </button>
        </div>
      ) : null}
      {issue ? (
        <p className="collection-error" role="alert">
          {t(failureKey(issue))}
        </p>
      ) : null}
      {selected && ['failed', 'cancelled'].includes(selected.state) && !importing ? (
        <p className="collection-error" role="status">
          {t(selected.state === 'cancelled' ? 'cancelled' : 'failed')}
        </p>
      ) : null}
      {!unitChoices.length && !units.loading ? (
        <div className="collection-empty">
          <BookOpen size={34} />
          <p>{t('emptyUnit')}</p>
          <button className="button primary" onClick={() => setEditing({ kind: 'unit' })}>
            {t('newUnit')}
          </button>
        </div>
      ) : null}
      {selected && !selected.prepareError && pages.data?.total === 0 && !importing ? (
        <div className="collection-empty">
          <FileText size={34} />
          <p>{t('emptyPage')}</p>
        </div>
      ) : null}
      <div className="collection-page-grid" aria-busy={pages.loading}>
        {pages.data?.items.map((page, index) => (
          <article className="collection-page-card" key={page.id}>
            <button
              disabled={!selected || importing || pages.loading}
              aria-label={t('pagePosition', {
                n: offset + index + 1,
                total: pages.data?.total ?? 0,
              })}
              onClick={() => selected && openReader(selected, offset + index)}
            >
              <Thumbnail id={page.id} kind="page" revision={revision} language={language} />
              <span>{offset + index + 1}</span>
            </button>
            <div>
              <button
                className="icon-button"
                aria-label={t('moveUp')}
                disabled={!selected || offset + index === 0 || busy || importing || pages.loading}
                onClick={() =>
                  void act(async () => {
                    const target = await library.pages(selected!, offset + index - 1, 1);
                    return library.movePage(selected!, page.id, target.items[0]?.id ?? null);
                  })
                }
              >
                <ArrowUp size={13} />
              </button>
              <button
                className="icon-button"
                aria-label={t('moveDown')}
                disabled={!selected || busy || importing || offset + index + 1 >= pages.data!.total}
                onClick={() =>
                  void act(async () => {
                    const target = await library.pages(selected!, offset + index + 2, 1);
                    return library.movePage(selected!, page.id, target.items[0]?.id ?? null);
                  })
                }
              >
                <ArrowDown size={13} />
              </button>
            </div>
          </article>
        ))}
      </div>
      {pages.data && pages.data.total > 48 ? (
        <div className="collection-pagination">
          <button
            className="button"
            disabled={offset === 0 || pages.loading}
            onClick={() => setOffset((n) => Math.max(0, n - 48))}
          >
            {t('previous')}
          </button>
          <span>{t('pagePosition', { n: offset + 1, total: pages.data.total })}</span>
          <button
            className="button"
            disabled={pages.data.nextOffset == null || pages.loading}
            onClick={() => setOffset(pages.data?.nextOffset ?? 0)}
          >
            {t('next')}
          </button>
        </div>
      ) : null}
      {sources.data?.length ? (
        <details className="collection-sources">
          <summary>
            {t('sources')} ({sources.data.length})
          </summary>
          {sources.data.map((source) => (
            <div key={source.id} className="collection-source-row">
              <span title={source.path}>{source.path}</span>
              <button
                className="icon-button"
                aria-label={t('openSource')}
                onClick={() => void act(() => library.openSource(source.id))}
              >
                <FileText size={15} />
              </button>
              <button
                className="icon-button"
                aria-label={t('reveal')}
                onClick={() => void act(() => library.openSource(source.id, true))}
              >
                <FolderOpen size={15} />
              </button>
              <button
                className="text-button"
                onClick={() =>
                  void act(async () => {
                    const result = await open({
                      directory: source.kind === 'folder',
                      multiple: false,
                    });
                    if (typeof result === 'string') await library.relink(source, result);
                  })
                }
              >
                {t('relink')}
              </button>
              <button
                className="icon-button"
                aria-label={t('remove')}
                onClick={() => void act(() => library.removeSource(source))}
              >
                <Trash2 size={15} />
              </button>
            </div>
          ))}
        </details>
      ) : null}
      <label className="collection-move-work">
        {t('moveCollection')}
        <select
          value={current.collectionId}
          disabled={busy}
          onChange={(event) => {
            const target = event.target.value;
            void act(async () => {
              await library.moveWork(current, target);
              onMove(target);
            });
          }}
        >
          {collections.map((item) => (
            <option key={item.id} value={item.id}>
              {item.title}
            </option>
          ))}
        </select>
      </label>
      {editing ? (
        <Editor
          key={editing.kind + ('item' in editing ? editing.item?.id : '')}
          editing={editing}
          language={language}
          onClose={() => setEditing(null)}
          onSave={async (values) => {
            if (editing.kind === 'remove') {
              await library.remove(editing.target, editing.item);
              if (editing.target === 'unit') {
                setRemovedUnits((previous) => new Set([...previous, editing.item.id]));
                setUnitId('');
              }
              if (editing.target === 'work') onBack();
            } else if (editing.kind === 'work')
              await library.updateWork(current, values.title, values.metadata);
            else if (editing.kind === 'unit') {
              const item = editing.item
                ? await library.updateUnit(editing.item, values.title, values.unitKind)
                : await library.createUnit(current.id, values.title, values.unitKind);
              setUnitId(item.id);
            }
            setEditing(null);
            reload();
          }}
        />
      ) : null}
      {reader ? (
        <PageReader
          work={current}
          unit={reader.unit}
          initialOffset={reader.offset}
          initialCompleted={reading.data?.completed ?? false}
          language={language}
          onRefresh={reload}
          onClose={() => {
            setReader(null);
            reload();
          }}
        />
      ) : null}
    </div>
  );
}
