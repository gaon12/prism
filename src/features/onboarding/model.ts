// Logic behind the first-run tour, kept free of React so it can be unit-tested.
//
// The tour edits a *draft* (OnboardingDraft). Nothing is saved until the last step, where
// `applyDraft` turns the draft into new ClientSettings that the app saves in one go. Skipping the
// tour therefore never leaves half-applied settings behind.

import { scopePathKey } from '../../lib/types';
import type {
  ClientSettings,
  Language,
  RecommendedFolder,
  SearchScope,
  Theme,
} from '../../lib/types';

/** The steps, in order. */
export const steps = ['welcome', 'shortcut', 'folders', 'privacy', 'ready'] as const;
export type StepId = (typeof steps)[number];

/** One folder offered on the "folders" step. */
export interface FolderChoice {
  path: string;
  name: string;
  /** Search this folder at all (file names). */
  selected: boolean;
  /** Also read the text inside documents in this folder. */
  content: boolean;
  /** Allow text from this folder to be sent to an external AI service. */
  external: boolean;
}

export interface OnboardingDraft {
  language: Language;
  theme: Theme;
  shortcut: string;
  folders: FolderChoice[];
  clipboardEnabled: boolean;
  clipboardImages: boolean;
  startOnLogin: boolean;
  /** Questions may go to an external AI service (off unless the user turns it on). */
  allowExternalQuestions: boolean;
}

/** Same folder written differently (case, slashes, trailing slash) counts as one folder. */
export function folderKey(path: string) {
  return scopePathKey(path.trim());
}

/** Starting draft: current settings, plus the suggested folders the user has not added yet. */
export function initialDraft(
  settings: ClientSettings,
  recommended: RecommendedFolder[],
): OnboardingDraft {
  const known = new Set(settings.scopes.map((scope) => folderKey(scope.path)));
  const seen = new Set<string>();
  const folders: FolderChoice[] = [];
  for (const folder of recommended) {
    const key = folderKey(folder.path);
    if (known.has(key) || seen.has(key)) continue;
    seen.add(key);
    // Suggested folders start ticked for file-name search only; reading document text and
    // sending anything outside the PC are always opt-in.
    folders.push({
      path: folder.path,
      name: folder.name,
      selected: true,
      content: false,
      external: false,
    });
  }
  return {
    language: settings.language,
    theme: settings.theme,
    shortcut: settings.shortcut,
    folders,
    clipboardEnabled: settings.clipboard.enabled,
    clipboardImages: settings.clipboard.captureImages,
    startOnLogin: settings.startOnLogin,
    allowExternalQuestions: settings.generation.allowExternalQuestions,
  };
}

/** Adds a folder the user picked by hand (ignored if it is already in the list). */
export function addFolder(draft: OnboardingDraft, path: string): OnboardingDraft {
  const key = folderKey(path);
  if (!key || draft.folders.some((folder) => folderKey(folder.path) === key)) return draft;
  const name =
    path
      .replace(/[\\/]+$/, '')
      .split(/[\\/]/)
      .pop() || path;
  return {
    ...draft,
    folders: [...draft.folders, { path, name, selected: true, content: false, external: false }],
  };
}

/**
 * Turns the finished draft into settings to save.
 * - Ticked folders are added as search scopes (existing scopes are kept untouched).
 * - Folder "external" permission only counts while external questions are allowed.
 * - The clipboard question counts as answered, so its separate notice is not shown again.
 */
export function applyDraft(
  settings: ClientSettings,
  draft: OnboardingDraft,
  newId: () => string = () => crypto.randomUUID(),
): ClientSettings {
  const known = new Set(settings.scopes.map((scope) => folderKey(scope.path)));
  const added: SearchScope[] = draft.folders.flatMap((folder) => {
    const key = folderKey(folder.path);
    if (!folder.selected || known.has(key)) return [];
    known.add(key);
    return [
      {
        id: newId(),
        name: folder.name,
        path: folder.path,
        filenameEnabled: true,
        contentEnabled: folder.content,
        externalAllowed: folder.content && draft.allowExternalQuestions && folder.external,
      },
    ];
  });
  return {
    ...settings,
    language: draft.language,
    theme: draft.theme,
    shortcut: draft.shortcut,
    startOnLogin: draft.startOnLogin,
    scopes: [...settings.scopes, ...added],
    clipboard: {
      ...settings.clipboard,
      enabled: draft.clipboardEnabled,
      captureImages: draft.clipboardEnabled && draft.clipboardImages,
      onboarded: true,
    },
    generation: { ...settings.generation, allowExternalQuestions: draft.allowExternalQuestions },
  };
}

/** The parts of a keyboard event the shortcut recorder needs. */
export interface KeyLike {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export type ShortcutResult =
  | { shortcut: string }
  /** Only modifier keys are held so far: keep waiting. */
  | { pending: true }
  /** A normal key without Ctrl/Alt/Win would block typing everywhere, so it is refused. */
  | { error: 'needsModifier' | 'unsupported' };

/** Keys a global shortcut may end with, by `KeyboardEvent.code`, in the app's spelling. */
function keyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const named: Record<string, string> = {
    Space: 'Space',
    Enter: 'Enter',
    Tab: 'Tab',
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
    Home: 'Home',
    End: 'End',
    PageUp: 'PageUp',
    PageDown: 'PageDown',
    Insert: 'Insert',
    Delete: 'Delete',
  };
  return named[code] ?? null;
}

const modifierCodes = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

/** Converts a key press into a shortcut such as "Ctrl+Shift+Space" (same format as settings). */
export function shortcutFromKey(event: KeyLike): ShortcutResult {
  if (modifierCodes.test(event.code)) return { pending: true };
  const key = keyName(event.code);
  if (!key) return { error: 'unsupported' };
  // Shift alone is not enough: Shift+A is just a capital letter while typing.
  if (!event.ctrlKey && !event.altKey && !event.metaKey) return { error: 'needsModifier' };
  const parts = [
    event.ctrlKey && 'Ctrl',
    event.altKey && 'Alt',
    event.shiftKey && 'Shift',
    event.metaKey && 'Super',
    key,
  ].filter(Boolean);
  return { shortcut: parts.join('+') };
}

/** Shortcut text for display: "Ctrl+Shift+Space" → ["Ctrl", "Shift", "Space"]. */
export function shortcutKeys(shortcut: string) {
  return shortcut
    .split('+')
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => (part === 'Super' ? 'Win' : part));
}
