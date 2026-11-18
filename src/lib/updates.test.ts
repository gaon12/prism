import { describe, expect, it } from 'vitest';
import { checkFailureKey } from './updates';

describe('failed update check message', () => {
  it('blames the connection only when the server could not be reached', () => {
    expect(checkFailureKey({ checkErrorKind: 'offline' })).toBe('checkFailed');
    expect(checkFailureKey({ checkErrorKind: 'server' })).toBe('checkServer');
    expect(checkFailureKey({ checkErrorKind: 'notConfigured' })).toBe('checkUnconfigured');
  });

  it('keeps the general message for older backends and failed calls', () => {
    expect(checkFailureKey({})).toBe('checkFailed');
    expect(checkFailureKey({ checkErrorKind: null })).toBe('checkFailed');
    expect(checkFailureKey(null)).toBe('checkFailed');
  });
});
