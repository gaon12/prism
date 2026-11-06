/** In-app file opening only. These keys are never registered as global shortcuts. */
type Chord = { ctrl: boolean; alt: boolean; shift: boolean; meta: boolean; key: string };
export type FileShortcutError =
  'fileShortcutInvalid' | 'fileShortcutReserved' | 'fileShortcutConflict';

function parse(input: string, platform?: string): Chord | null {
  if (!input || input.length > 64 || !/^[\x20-\x7e]+$/.test(input)) return null;
  const parts = input.split('+').map((part) => part.trim().toLowerCase());
  const key = parts.pop()!;
  const chord: Chord = { ctrl: false, alt: false, shift: false, meta: false, key: '' };
  for (const part of parts) {
    const modifier =
      part === 'ctrl' || part === 'control'
        ? 'ctrl'
        : part === 'alt' || part === 'option'
          ? 'alt'
          : part === 'shift'
            ? 'shift'
            : ['super', 'meta', 'cmd', 'command'].includes(part)
              ? 'meta'
              : ['cmdorctrl', 'commandorcontrol', 'commandorctrl', 'cmdorcontrol'].includes(part)
                ? platform === 'macos'
                  ? 'meta'
                  : 'ctrl'
                : null;
    if (!modifier || chord[modifier]) return null;
    chord[modifier] = true;
  }
  chord.key = ['enter', 'return', 'numpadenter'].includes(key)
    ? 'Enter'
    : key === 'space'
      ? 'Space'
      : /^(?:key)?[a-z]$/.test(key)
        ? `Key${key.at(-1)!.toUpperCase()}`
        : /^(?:digit)?[0-9]$/.test(key)
          ? `Digit${key.at(-1)}`
          : /^f(?:[1-9]|1[0-9]|2[0-4])$/.test(key)
            ? key.toUpperCase()
            : '';
  return chord.key ? chord : null;
}

function canonical(chord: Chord): string {
  return [
    chord.ctrl && 'Ctrl',
    chord.alt && 'Alt',
    chord.shift && 'Shift',
    chord.meta && 'Super',
    chord.key,
  ]
    .filter(Boolean)
    .join('+');
}

/** Same whitelist/reservations as settings::file_shortcut on the native side. */
export function fileShortcutError(
  input: string,
  global = '',
  platform?: string,
): FileShortcutError | null {
  const chord = parse(input, platform);
  if (!chord) return 'fileShortcutInvalid';
  if (
    (!chord.ctrl &&
      !chord.alt &&
      !chord.meta &&
      chord.key !== 'Enter' &&
      !/^F\d+$/.test(chord.key)) ||
    ((chord.ctrl || chord.meta) && chord.key === 'Enter') ||
    (chord.alt && chord.key === 'F4') ||
    ((chord.ctrl || chord.meta) && ['KeyQ', 'KeyW'].includes(chord.key))
  )
    return 'fileShortcutReserved';
  const quick = parse(global, platform);
  return quick && canonical(quick) === canonical(chord) ? 'fileShortcutConflict' : null;
}

/** Keep aliases/extra spaces from crowding the search field and quick footer. */
export function fileShortcutLabel(input: string, platform?: string): string {
  const chord = parse(input, platform);
  return chord ? canonical(chord).replace(/Key(?=[A-Z]$)|Digit(?=\d$)/, '') : input;
}

type KeyEvent = Pick<
  KeyboardEvent,
  | 'key'
  | 'code'
  | 'ctrlKey'
  | 'altKey'
  | 'shiftKey'
  | 'metaKey'
  | 'isComposing'
  | 'keyCode'
  | 'repeat'
  | 'getModifierState'
>;

export function matchesFileShortcut(
  input: string,
  event: KeyEvent,
  platform?: string,
  includeRepeats = false,
): boolean {
  if (
    (event.repeat && !includeRepeats) ||
    event.isComposing ||
    event.keyCode === 229 ||
    event.getModifierState?.('AltGraph')
  )
    return false;
  if (fileShortcutError(input, '', platform)) return false;
  const chord = parse(input, platform)!;
  // Physical Latin codes still identify Ctrl+O while a Korean/Japanese IME is selected.
  const key =
    event.code === 'NumpadEnter'
      ? 'Enter'
      : event.code || (event.key === ' ' ? 'Space' : parse(event.key, platform)?.key);
  return (
    chord.key === key &&
    chord.ctrl === event.ctrlKey &&
    chord.alt === event.altKey &&
    chord.shift === event.shiftKey &&
    chord.meta === event.metaKey
  );
}
