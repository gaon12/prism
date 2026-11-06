import { invoke } from '@tauri-apps/api/core';
import { textDocument } from './text-document';
export type WorkKind = 'document' | 'image' | 'comic' | 'webtoon' | 'novel' | 'hitomi' | 'other';
export type Direction = 'leftToRight' | 'rightToLeft' | 'vertical';
export type UnitKind = 'volume' | 'chapter' | 'part' | 'document';
export interface Collection {
  id: string;
  title: string;
  description: string;
  revision: number;
  createdMs: number;
  updatedMs: number;
}
export interface Metadata {
  kind: WorkKind;
  direction: Direction;
  authors: string[];
  tags: string[];
  description: string;
  externalId: string | null;
}
export interface Work {
  id: string;
  collectionId: string;
  title: string;
  metadata: Metadata;
  position: number;
  revision: number;
  unitCount: number;
  pageCount: number;
}
export interface Unit {
  id: string;
  workId: string;
  title: string;
  kind: UnitKind;
  position: number;
  revision: number;
  state: string;
  pageCount: number;
}
export interface Source {
  id: string;
  unitId: string;
  path: string;
  kind: string;
  revision: number;
  position: number;
}
export interface Page {
  id: string;
  unitId: string;
  sourceId: string;
  position: number;
  location: {
    kind: 'image' | 'pdf' | 'text' | 'epub' | 'office';
    entry: string | null;
    number: number;
    renderer?: string;
  };
  title: string;
  sourceHash: string;
}
export interface Listing<T> {
  items: T[];
  total: number;
  nextOffset: number | null;
}
export interface Media {
  pageId: string;
  sourceHash: string;
  dataUrl: string | null;
  text: string | null;
  document: EpubDocument | null;
}
export interface EpubDocument {
  html: string;
  width: number;
  height: number;
  fixed: boolean;
}
export interface Reading {
  workId: string;
  pageId: string | null;
  completed: boolean;
  updatedMs: number;
}
export interface NavigationItem {
  title: string;
  depth: number;
  pageId: string | null;
  offset: number | null;
  fragment: string | null;
}
export interface Navigation {
  unitId: string;
  unitRevision: number;
  sourceHash: string;
  items: NavigationItem[];
}
export interface Progress {
  unitId: string;
  phase: string;
  processed: number;
  error: string | null;
}
export const defaults: Metadata = {
  kind: 'document',
  direction: 'leftToRight',
  authors: [],
  tags: [],
  description: '',
  externalId: null,
};
export const library = {
  get: (id: string) => invoke<Collection>('collection_get', { id }),
  work: (id: string) => invoke<Work>('collection_work_get', { id }),
  unit: (id: string) => invoke<Unit>('collection_unit_get', { id }),
  list: (offset = 0) => invoke<Listing<Collection>>('collection_list', { offset, limit: 100 }),
  create: (title: string, description: string) =>
    invoke<Collection>('collection_create', { title, description }),
  update: (item: Collection, title: string, description: string) =>
    invoke<Collection>('collection_update', {
      id: item.id,
      expected: item.revision,
      title,
      description,
    }),
  works: (collectionId: string, offset = 0) =>
    invoke<Listing<Work>>('collection_works', { collectionId, offset, limit: 24 }),
  createWork: (collectionId: string, title: string, metadata: Metadata) =>
    invoke<Work>('collection_work_create', { collectionId, title, metadata }),
  updateWork: (item: Work, title: string, metadata: Metadata) =>
    invoke<Work>('collection_work_update', {
      id: item.id,
      expected: item.revision,
      title,
      metadata,
    }),
  units: (workId: string, offset = 0, limit = 100) =>
    invoke<Listing<Unit>>('collection_units', { workId, offset, limit }),
  createUnit: (workId: string, title: string, kind: UnitKind) =>
    invoke<Unit>('collection_unit_create', { workId, title, kind }),
  updateUnit: (item: Unit, title: string, kind: UnitKind) =>
    invoke<Unit>('collection_unit_update', { id: item.id, expected: item.revision, title, kind }),
  sources: (unitId: string) => invoke<Source[]>('collection_sources', { unitId }),
  import: (item: Unit, paths: string[]) =>
    invoke<Unit>('collection_import', { unitId: item.id, expected: item.revision, paths }),
  cancel: (unitId: string) => invoke<void>('collection_cancel', { unitId }),
  jobs: () => invoke<Progress[]>('collection_jobs'),
  pages: (item: Unit, offset = 0, limit = 48) =>
    invoke<Listing<Page>>('collection_pages', {
      unitId: item.id,
      expected: item.revision,
      offset,
      limit,
    }),
  address: (workId: string, pageId: string) =>
    invoke<{ page: Page; unit: Unit; offset: number }>('collection_page_address', {
      workId,
      pageId,
    }),
  media: async (pageId: string, thumbnail = false) =>
    completeMedia(await invoke<Media>('collection_media', { pageId, thumbnail }), thumbnail),
  navigation: (pageId: string, expected: number) =>
    invoke<Navigation>('collection_navigation', { pageId, expected }),
  cover: async (workId: string) =>
    completeMedia(await invoke<Media>('collection_cover', { workId }), true),
  reading: (workId: string) => invoke<Reading>('collection_reading', { workId }),
  saveReading: (workId: string, pageId: string | null, completed: boolean) =>
    serialReading(workId, () =>
      invoke<Reading>('collection_save_reading', { workId, pageId, completed }),
    ),
  remove: (kind: 'collection' | 'work' | 'unit', item: { id: string; revision: number }) =>
    invoke<void>('collection_remove', { kind, id: item.id, expected: item.revision }),
  moveWork: (item: Work, collectionId: string) =>
    invoke<Work>('collection_move_work', { id: item.id, expected: item.revision, collectionId }),
  moveUnit: (item: Unit, before: string | null) =>
    invoke<Unit>('collection_move_unit', { id: item.id, expected: item.revision, before }),
  movePage: (item: Unit, pageId: string, before: string | null) =>
    invoke<Unit>('collection_move_page', {
      unitId: item.id,
      expected: item.revision,
      pageId,
      before,
    }),
  relink: (item: Source, path: string) =>
    invoke<Source>('collection_relink', { id: item.id, expected: item.revision, path }),
  removeSource: (item: Source) =>
    invoke<void>('collection_remove_source', { id: item.id, expected: item.revision }),
  openSource: (id: string, reveal = false) =>
    invoke<void>('collection_open_source', { id, reveal }),
};
async function completeMedia(media: Media, thumbnail: boolean): Promise<Media> {
  if (!thumbnail || media.dataUrl || (!media.document && media.text === null)) return media;
  const { epubThumbnail } = await import('./epub-render');
  const document = media.document ?? textDocument(media.text!);
  const dataUrl = await epubThumbnail(document);
  return invoke<Media>(media.document ? 'collection_cache_epub' : 'collection_cache_text', {
    pageId: media.pageId,
    sourceHash: media.sourceHash,
    dataUrl,
  });
}
const readingWrites = new Map<string, Promise<unknown>>();
function serialReading<T>(key: string, operation: () => Promise<T>): Promise<T> {
  const next = (readingWrites.get(key) ?? Promise.resolve()).catch(() => {}).then(operation);
  readingWrites.set(key, next);
  void next
    .finally(() => {
      if (readingWrites.get(key) === next) readingWrites.delete(key);
    })
    .catch(() => {});
  return next;
}
export function readingSettled(workId: string) {
  return readingWrites.get(workId) ?? Promise.resolve();
}
// Keep at most two native media IPC requests outstanding. Unmounted cards do
// not leave hundreds of decoder tasks queued in the native blocking pool.
let active = 0;
const waiters: Array<() => void> = [];
export function mediaTask<T>(
  operation: () => Promise<T>,
  cancelled: () => boolean,
  priority = false,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const start = () => {
      if (cancelled()) {
        reject({ code: 'cancelled' });
        waiters.shift()?.();
        return;
      }
      active++;
      void Promise.resolve()
        .then(operation)
        .then(resolve, reject)
        .finally(() => {
          active--;
          waiters.shift()?.();
        });
    };
    if (active < 2) start();
    else if (priority) waiters.unshift(start);
    else waiters.push(start);
  });
}
