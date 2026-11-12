import { useEffect, useRef, useState } from 'react';

/**
 * While the filename index is still scanning, how often a visible search may
 * re-run to show newly found files.
 *
 * Every published batch raises the index revision (the UI polls it every
 * 400 ms). Re-running the search on each one is expensive on a small PC: an
 * empty or broad query ranks the whole catalog (about one second for two
 * million files on one core), and that search holds the catalog read lock,
 * so the scan itself has to wait. Measured on a 1 CPU / 512 MB job, the
 * whole C: drive took 18.8 s to list with the window open versus 9.7 s
 * without it.
 */
export const SCAN_REFRESH_MS = 2000;

/**
 * Milliseconds to wait before the search may follow a new revision.
 * Outside a scan the answer is always 0, and so is the first refresh of a
 * scan, so the first results and the finished list appear at once.
 */
export function scanRefreshDelay(lastRefreshAt: number | null, now: number, scanning: boolean) {
  if (!scanning || lastRefreshAt === null) return 0;
  return Math.max(0, lastRefreshAt + SCAN_REFRESH_MS - now);
}

/**
 * The index revision a search should follow: the live revision when idle,
 * and at most one new revision per SCAN_REFRESH_MS while scanning. A burst of
 * revisions keeps the same deadline, so updates are delayed but never starved.
 */
export function useScanRefreshRevision(revision: number, scanning: boolean) {
  const [followed, setFollowed] = useState(revision);
  const lastRefreshAt = useRef<number | null>(null);
  useEffect(() => {
    if (revision === followed) return;
    const follow = () => {
      lastRefreshAt.current = scanning ? Date.now() : null;
      setFollowed(revision);
    };
    const wait = scanRefreshDelay(lastRefreshAt.current, Date.now(), scanning);
    if (wait === 0) {
      follow();
      return;
    }
    const timer = setTimeout(follow, wait);
    return () => clearTimeout(timer);
  }, [revision, scanning, followed]);
  return followed;
}
