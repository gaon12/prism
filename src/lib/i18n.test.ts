import { describe, expect, it } from 'vitest';
import { startupFailure } from './i18n';
import type { Language } from './types';

describe('Windows startup failure messages', () => {
  it('gives user and policy errors distinct guidance in each supported language', () => {
    for (const language of ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'] satisfies Language[]) {
      const user = startupFailure(language, 'STARTUP_DISABLED_BY_USER');
      const policy = startupFailure(language, 'STARTUP_DISABLED_BY_POLICY');
      expect(user).not.toContain('STARTUP_');
      expect(policy).not.toContain('STARTUP_');
      expect(user).not.toBe(policy);
      expect(startupFailure(language, 'STARTUP_UNAVAILABLE')).not.toContain('STARTUP_');
    }
  });

  it('preserves unrelated errors so existing diagnostics remain available', () => {
    expect(startupFailure('ko', 'Settings revision mismatch.')).toBe('Settings revision mismatch.');
  });
});
