import { describe, expect, it } from 'vitest';
import { SCAN_REFRESH_MS, scanRefreshDelay } from './scan-refresh';

describe('scanRefreshDelay', () => {
  it('follows every revision at once outside a scan', () => {
    expect(scanRefreshDelay(null, 1000, false)).toBe(0);
    expect(scanRefreshDelay(900, 1000, false)).toBe(0);
  });

  it('shows the first results of a scan at once', () => {
    expect(scanRefreshDelay(null, 1000, true)).toBe(0);
  });

  it('waits out the rest of the interval during a scan, never negative', () => {
    expect(scanRefreshDelay(1000, 1500, true)).toBe(SCAN_REFRESH_MS - 500);
    expect(scanRefreshDelay(1000, 1000 + SCAN_REFRESH_MS, true)).toBe(0);
    expect(scanRefreshDelay(1000, 1000 + SCAN_REFRESH_MS * 3, true)).toBe(0);
  });
});
