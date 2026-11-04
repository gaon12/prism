import { describe, expect, it } from 'vitest';
import { defaultSettings } from '../../lib/types';
import type { ClientSettings } from '../../lib/types';
import { addFolder, applyDraft, initialDraft, shortcutFromKey, shortcutKeys } from './model';

/** Fresh settings with one folder the user already added. */
function settings(): ClientSettings {
  const base = structuredClone(defaultSettings);
  base.scopes = [
    {
      id: 'docs',
      name: 'Documents',
      path: 'C:\\Users\\me\\Documents',
      filenameEnabled: true,
      contentEnabled: true,
      externalAllowed: false,
    },
  ];
  return base;
}

const key = (
  code: string,
  mods: Partial<Record<'ctrl' | 'alt' | 'shift' | 'meta', boolean>> = {},
) => ({
  code,
  ctrlKey: !!mods.ctrl,
  altKey: !!mods.alt,
  shiftKey: !!mods.shift,
  metaKey: !!mods.meta,
});

describe('onboarding draft', () => {
  it('preserves case-sensitive POSIX folders and never grants content egress for filename-only scopes', () => {
    const current = settings();
    const draft = initialDraft(current, [
      { name: 'Upper', path: '/home/Notes' },
      { name: 'Lower', path: '/home/notes' },
    ]);
    expect(draft.folders).toHaveLength(2);
    draft.allowExternalQuestions = true;
    draft.folders[0].external = true;
    draft.folders.push({ ...draft.folders[0] });
    const after = applyDraft(current, draft);
    expect(after.scopes.slice(1)).toHaveLength(2);
    expect(after.scopes.slice(1).every((scope) => !scope.externalAllowed)).toBe(true);
  });
  it('offers only suggested folders that are not added yet, file names only', () => {
    const draft = initialDraft(settings(), [
      { name: 'Documents', path: 'c:/users/me/documents/' },
      { name: 'Desktop', path: 'C:\\Users\\me\\Desktop' },
      { name: 'Desktop', path: 'C:\\Users\\me\\Desktop\\' },
    ]);
    expect(draft.folders).toEqual([
      {
        path: 'C:\\Users\\me\\Desktop',
        name: 'Desktop',
        selected: true,
        content: false,
        external: false,
      },
    ]);
    expect(draft.allowExternalQuestions).toBe(false);
  });

  it('adds hand-picked folders once, named after the last path part', () => {
    let draft = initialDraft(settings(), []);
    draft = addFolder(draft, 'D:\\Work\\Reports\\');
    draft = addFolder(draft, 'd:/work/reports');
    expect(draft.folders.map((folder) => folder.name)).toEqual(['Reports']);
  });

  it('applies choices, keeps existing scopes and marks the clipboard question answered', () => {
    const before = settings();
    const draft = {
      ...initialDraft(before, [{ name: 'Desktop', path: 'C:\\Users\\me\\Desktop' }]),
      language: 'ja' as const,
      shortcut: 'Ctrl+Shift+Space',
      clipboardEnabled: false,
      clipboardImages: true,
    };
    draft.folders = [
      ...draft.folders,
      { path: 'E:\\Skip', name: 'Skip', selected: false, content: true, external: true },
    ];
    const after = applyDraft(before, draft, () => 'new-id');
    expect(after.language).toBe('ja');
    expect(after.shortcut).toBe('Ctrl+Shift+Space');
    expect(after.scopes.map((scope) => scope.id)).toEqual(['docs', 'new-id']);
    expect(after.scopes[1]).toMatchObject({ filenameEnabled: true, contentEnabled: false });
    // Images are only captured while clipboard history itself is on.
    expect(after.clipboard).toMatchObject({
      enabled: false,
      captureImages: false,
      onboarded: true,
    });
    expect(before.clipboard.onboarded).toBe(false);
  });

  it('only lets a folder send text outside the PC while external questions are allowed', () => {
    const base = initialDraft(settings(), [{ name: 'Notes', path: 'C:\\Notes' }]);
    base.folders[0] = { ...base.folders[0], content: true, external: true };
    const off = applyDraft(settings(), base, () => 'n');
    expect(off.scopes[1].externalAllowed).toBe(false);
    expect(off.generation.allowExternalQuestions).toBe(false);
    const on = applyDraft(settings(), { ...base, allowExternalQuestions: true }, () => 'n');
    expect(on.scopes[1].externalAllowed).toBe(true);
    expect(on.generation.allowExternalQuestions).toBe(true);
  });
});

describe('shortcut recorder', () => {
  it('builds shortcuts in the settings format', () => {
    expect(shortcutFromKey(key('Space', { alt: true }))).toEqual({ shortcut: 'Alt+Space' });
    expect(shortcutFromKey(key('KeyK', { ctrl: true, shift: true }))).toEqual({
      shortcut: 'Ctrl+Shift+K',
    });
    expect(shortcutFromKey(key('F12', { meta: true }))).toEqual({ shortcut: 'Super+F12' });
    expect(shortcutFromKey(key('ArrowUp', { ctrl: true, alt: true }))).toEqual({
      shortcut: 'Ctrl+Alt+Up',
    });
  });

  it('waits on modifiers and refuses shortcuts that would block typing', () => {
    expect(shortcutFromKey(key('ControlLeft', { ctrl: true }))).toEqual({ pending: true });
    expect(shortcutFromKey(key('KeyA'))).toEqual({ error: 'needsModifier' });
    expect(shortcutFromKey(key('KeyA', { shift: true }))).toEqual({ error: 'needsModifier' });
    expect(shortcutFromKey(key('Semicolon', { ctrl: true }))).toEqual({ error: 'unsupported' });
  });

  it('splits shortcuts into key caps for display', () => {
    expect(shortcutKeys('Ctrl+Super+Space')).toEqual(['Ctrl', 'Win', 'Space']);
  });
});
