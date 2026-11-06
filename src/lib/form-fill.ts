// Filling an existing DOCX/HWPX form (requirement Q46).
//
// The user picks the original form; Prism lists its fields ({{name}} placeholders and ___ blanks),
// the user types values, and Prism writes a *new* file next to it. The original is never changed.
// The native side (src-tauri/src/form_fill.rs) does all reading and writing.
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { native } from './bridge';

export type FormFormat = 'docx' | 'hwpx';

export interface FormField {
  /** Placeholder name, or `blank:N` for the N-th underscore blank. */
  key: string;
  kind: 'placeholder' | 'blank';
  /** Text around the field, to show where it is in the form. */
  context: string;
}

export interface FormInfo {
  path: string;
  format: FormFormat;
  fields: FormField[];
  /** SHA-256 of the file when it was read; a save is refused if the file changed since. */
  fingerprint: string;
}

/** Error codes the native side reports. */
export type FormError =
  | 'input'
  | 'unsupported'
  | 'damaged'
  | 'tooLarge'
  | 'invalid'
  | 'changed'
  | 'timedOut'
  | 'busy'
  | 'io'
  | 'unavailable';

/** Longest value accepted per field (the native limit is 64 KiB of UTF-8). */
export const maxValueLength = 16_000;

const extensions = ['docx', 'hwpx'];

export const formFill = {
  /** Reads the same form again (after it changed on disk). */
  async reload(path: string): Promise<FormInfo> {
    if (!native) throw 'unavailable' satisfies FormError;
    return invoke<FormInfo>('form_fields', { path });
  },

  /** Asks for a form file and lists its fields; null when the user cancels. */
  async choose(): Promise<FormInfo | null> {
    if (!native) throw 'unavailable' satisfies FormError;
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: 'DOCX / HWPX', extensions }],
    });
    if (typeof path !== 'string') return null;
    return invoke<FormInfo>('form_fields', { path });
  },

  /** Asks where to save, then writes the filled copy. Returns the saved path, or null. */
  async fill(form: FormInfo, values: Record<string, string>): Promise<string | null> {
    if (!native) throw 'unavailable' satisfies FormError;
    const destination = await save({
      defaultPath: filledName(form.path),
      filters: [{ name: form.format.toUpperCase(), extensions: [form.format] }],
    });
    if (!destination) return null;
    // Only non-empty values are sent; an empty field stays as it was in the form.
    const filled = Object.fromEntries(
      Object.entries(values).filter(([, value]) => value.trim().length > 0),
    );
    await invoke('form_fill', {
      source: form.path,
      destination,
      values: filled,
      fingerprint: form.fingerprint,
    });
    return destination;
  },
};

/** "C:\Forms\form.hwpx" → "form (filled).hwpx": a default name that never overwrites the original. */
export function filledName(path: string) {
  const file = path.split(/[\\/]/).pop() ?? 'form.docx';
  const dot = file.lastIndexOf('.');
  return dot > 0 ? `${file.slice(0, dot)} (filled)${file.slice(dot)}` : `${file} (filled)`;
}
