import { describe, expect, it } from 'vitest';
import { indexService, indexServiceIssue } from './index-service';

describe('index-service client boundary', () => {
  it('keeps browser previews read-only and rejects administrative actions', async () => {
    expect(await indexService.status()).toMatchObject({
      state: 'unsupported',
      canInstall: false,
      canStart: false,
      canRemove: false,
    });
    for (const action of ['install', 'start', 'uninstall'] as const) {
      await expect(indexService.manage(action)).rejects.toEqual({ code: 'unsupported' });
    }
  });
  it('accepts only public issue codes and never displays arbitrary backend details', () => {
    for (const code of [
      'cancelled',
      'busy',
      'permissionDenied',
      'unsafeLocation',
      'isolated',
      'timedOut',
    ]) {
      expect(
        indexServiceIssue({ code, message: 'PRIVATE_BACKEND_TEXT', path: 'PRIVATE_PATH' }),
      ).toBe(code);
    }
    for (const error of [
      null,
      undefined,
      'PRIVATE_BACKEND_TEXT',
      { code: 'PRIVATE_BACKEND_TEXT' },
      new Error('PRIVATE_BACKEND_TEXT'),
    ]) {
      expect(indexServiceIssue(error)).toBe('unavailable');
    }
  });
});
