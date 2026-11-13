/**
 * How many filename result rows the list actually draws.
 *
 * A search returns up to 200 rows, but a window shows only a dozen or so.
 * Drawing all of them costs about 33 DOM nodes each (about 6,600 nodes), and
 * while a first scan runs the list is replaced every two seconds. On a
 * 512 MB machine that alone grew the window's WebView process by 20-30 MB.
 * The list therefore draws one page and adds pages as the user scrolls, or
 * as far as the keyboard selection goes.
 */
export const RESULT_PAGE = 40;

/** Extra rows drawn below the selection, so the next arrow press has a row. */
const SELECTION_MARGIN = 10;

/**
 * Rows to draw out of `total`: at least `shown` (what scrolling has
 * opened), and always enough to include the selected row plus a margin.
 * `selected` is the selected row's index, or -1 for none.
 */
export function drawnRows(total: number, shown: number, selected: number): number {
  const forSelection = selected >= 0 ? selected + 1 + SELECTION_MARGIN : 0;
  return Math.max(0, Math.min(total, Math.max(shown, forSelection)));
}

/**
 * The folder that holds a result: its path without the name, so the folder column shows only
 * what the name column does not and a row stays on one line. A drive or file-system root keeps
 * its separator (`C:\`, `/`).
 */
export function parentFolder(path: string, name: string): string {
  if (!name || !path.endsWith(name)) return path;
  const rest = path.slice(0, path.length - name.length);
  const folder = rest.replace(/[\\/]+$/, '');
  return !folder || /^[A-Za-z]:$/.test(folder) ? rest : folder;
}
