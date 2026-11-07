import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./bridge', () => ({ native: true }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { formFill, type FormInfo } from './form-fill';

const form = (path: string): FormInfo => ({
  path,
  format: path.endsWith('.hwpx') ? 'hwpx' : 'docx',
  fields: [],
  fingerprint: 'original-source-sha256',
});

beforeEach(() => vi.resetAllMocks());

describe('native form save destination', () => {
  it.each([
    ['/home/user/Forms/양식 😀.docx', '/home/user/Forms/양식 😀 (filled).docx'],
    ['/Users/user/Forms/form.v2.hwpx', '/Users/user/Forms/form.v2 (filled).hwpx'],
    ['/home/user/Forms/literal\\name.docx', '/home/user/Forms/literal\\name (filled).docx'],
    ['C:\\Forms\\양식.docx', 'C:\\Forms\\양식 (filled).docx'],
    ['C:/Forms/form.hwpx', 'C:/Forms/form (filled).hwpx'],
    ['\\\\server\\share\\Forms\\form.docx', '\\\\server\\share\\Forms\\form (filled).docx'],
  ])('suggests a separate copy in the source folder for %s', async (source, expected) => {
    vi.mocked(save).mockResolvedValue(null);
    expect(await formFill.fill(form(source), { 이름: '홍길동' })).toBeNull();
    expect(save).toHaveBeenCalledWith({
      defaultPath: expected,
      filters: source.endsWith('.hwpx')
        ? [{ name: 'HWPX', extensions: ['hwpx'] }]
        : [{ name: 'DOCX', extensions: ['docx'] }],
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('uses the user-chosen destination and retains the source fingerprint', async () => {
    const source = '/home/user/Forms/form.docx';
    const destination = '/home/user/Exports/다른 이름.docx';
    vi.mocked(save).mockResolvedValue(destination);
    vi.mocked(invoke).mockResolvedValue(undefined);
    expect(await formFill.fill(form(source), { 이름: '홍길동', 비움: '  ' })).toBe(destination);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('form_fill', {
      source,
      destination,
      values: { 이름: '홍길동' },
      fingerprint: 'original-source-sha256',
    });
  });
});
