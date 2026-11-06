import { describe, expect, it } from 'vitest';
import cases from '../../tests/fixtures/file-shortcuts.json';
import { fileShortcutError, fileShortcutLabel, matchesFileShortcut } from './file-shortcut';

const event = (changes: Partial<KeyboardEvent> = {}) => ({
  key: 'o',
  code: 'KeyO',
  ctrlKey: true,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  isComposing: false,
  keyCode: 79,
  repeat: false,
  getModifierState: () => false,
  ...changes,
});

describe('configurable file/folder opening', () => {
  it('shows compact chord labels without changing the stored alias', () => {
    expect(fileShortcutLabel('  Shift + Control + KeyO  ')).toBe('Ctrl+Shift+O');
    expect(fileShortcutLabel('CommandOrControl+Digit2', 'macos')).toBe('Super+2');
  });
  for (const item of cases) {
    it(`shares native validation: ${JSON.stringify(item.input)} (${item.mac ? 'Mac' : 'other'})`, () => {
      expect(fileShortcutError(item.input, '', item.mac ? 'macos' : 'windows')).toBe(
        item.error ?? null,
      );
      if (item.canonical) {
        expect(fileShortcutError(item.input, item.canonical, item.mac ? 'macos' : 'windows')).toBe(
          'fileShortcutConflict',
        );
      }
    });
  }
  it('matches physical keys with a Korean IME but never during composition, repeats or AltGr', () => {
    expect(matchesFileShortcut('Ctrl+O', event({ key: 'ㅐ' }))).toBe(true);
    for (const change of [
      { isComposing: true },
      { keyCode: 229 },
      { repeat: true },
      { getModifierState: () => true },
    ]) {
      expect(matchesFileShortcut('Ctrl+O', event(change))).toBe(false);
    }
    expect(matchesFileShortcut('Ctrl+O', event({ shiftKey: true }))).toBe(false);
    expect(matchesFileShortcut('Ctrl+O', event({ ctrlKey: false }))).toBe(false);
    expect(matchesFileShortcut('Ctrl+O', event({ code: 'KeyP' }))).toBe(false);
    // Handlers can consume a held shortcut without running another opening action.
    expect(matchesFileShortcut('Ctrl+O', event({ repeat: true }), 'windows', true)).toBe(true);
  });
  it('resolves CommandOrControl per platform and supports numeric/function/keypad keys', () => {
    expect(matchesFileShortcut('CmdOrCtrl+O', event(), 'windows')).toBe(true);
    expect(matchesFileShortcut('CmdOrCtrl+O', event(), 'linux')).toBe(true);
    expect(matchesFileShortcut('CmdOrCtrl+O', event(), 'macos')).toBe(false);
    expect(
      matchesFileShortcut('CmdOrCtrl+O', event({ ctrlKey: false, metaKey: true }), 'macos'),
    ).toBe(true);
    expect(
      matchesFileShortcut('Enter', event({ key: 'Enter', code: 'NumpadEnter', ctrlKey: false })),
    ).toBe(true);
    expect(matchesFileShortcut('Enter', event())).toBe(false);
    expect(matchesFileShortcut('F4', event({ key: 'F4', code: 'F4', ctrlKey: false }))).toBe(true);
    expect(matchesFileShortcut('Ctrl+2', event({ key: '@', code: 'Digit2' }))).toBe(true);
  });
  it('detects reordered/aliased global conflicts while preserving quick navigation and chat keys', () => {
    expect(fileShortcutError('Shift+Control+O', 'Ctrl+Shift+KeyO')).toBe('fileShortcutConflict');
    expect(fileShortcutError('Alt+Space', 'alt+space')).toBe('fileShortcutConflict');
    expect(fileShortcutError('Ctrl+O', 'Alt+Space')).toBeNull();
    expect(matchesFileShortcut('Ctrl+Enter', event({ code: 'Enter', key: 'Enter' }))).toBe(false);
    expect(
      matchesFileShortcut(
        'Shift+Ctrl+Enter',
        event({ code: 'Enter', key: 'Enter', shiftKey: true }),
      ),
    ).toBe(false);
  });
});
