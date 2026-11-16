import type { FileResult } from './types';
import { parentFolder } from './result-window';

/**
 * How the filename result list is ordered. `null` keeps the search's own order (best match
 * first). Sorting only reorders the rows the search returned (up to 200); it does not search
 * again or change which files match.
 */
export type FileSortKey = 'name' | 'folder' | 'size' | 'modified';
export type FileSort = { key: FileSortKey; descending: boolean } | null;

/**
 * The order after a column heading is pressed: first ascending, then descending, then back
 * to best match. Pressing another column starts that column ascending.
 */
export function nextSort(current: FileSort, key: FileSortKey): FileSort {
  if (current?.key !== key) return { key, descending: false };
  return current.descending ? null : { key, descending: true };
}

// Natural order, so "file2" comes before "file10", and case does not split the list.
const text = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/**
 * A sorted copy of `results`. Ties keep the search's own order, so equal names stay ranked
 * by match quality. Folders have no size, so they come before files when sorting by size.
 * Files with an unknown modification time always go last, in either direction.
 */
export function sortResults(results: FileResult[], sort: FileSort): FileResult[] {
  if (!sort) return results;
  const direction = sort.descending ? -1 : 1;
  const compare = (left: FileResult, right: FileResult): number => {
    switch (sort.key) {
      case 'name':
        return direction * text.compare(left.name, right.name);
      case 'folder':
        return (
          direction *
          text.compare(parentFolder(left.path, left.name), parentFolder(right.path, right.name))
        );
      case 'size':
        return direction * ((left.isDir ? -1 : left.size) - (right.isDir ? -1 : right.size));
      case 'modified':
        if (left.modifiedMs === null || right.modifiedMs === null)
          return (left.modifiedMs === null ? 1 : 0) - (right.modifiedMs === null ? 1 : 0);
        return direction * (left.modifiedMs - right.modifiedMs);
    }
  };
  return results
    .map((file, rank) => ({ file, rank }))
    .sort((left, right) => compare(left.file, right.file) || left.rank - right.rank)
    .map(({ file }) => file);
}
