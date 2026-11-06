// "Fill an existing form" panel (requirement Q46).
//
// 1. Open a DOCX/HWPX form → the native side lists its fields.
// 2. Type a value for each field (placeholders with the same name are filled together).
// 3. Save → a new file is written; the original form is never modified.
import { useId, useState } from 'react';
import { FileInput, Save } from 'lucide-react';
import { formFill, maxValueLength, type FormError, type FormInfo } from '../../lib/form-fill';
import type { Language } from '../../lib/types';
import { formFillText, type FormFillMessage } from './form-fill-messages';
import './form-fill.css';

const errors: FormError[] = [
  'input',
  'unsupported',
  'damaged',
  'tooLarge',
  'invalid',
  'changed',
  'timedOut',
  'busy',
  'io',
  'unavailable',
];

/** Maps whatever the native call threw to one of our messages. */
function errorKey(error: unknown): FormFillMessage {
  const code = typeof error === 'string' ? error : (error as { code?: string } | null)?.code;
  return errors.includes(code as FormError) ? (code as FormError) : 'io';
}

export function FormFillPanel({ language }: { language: Language }) {
  const t = (key: FormFillMessage, values?: Record<string, string | number>) =>
    formFillText(language, key, values);
  const id = useId();
  const [form, setForm] = useState<FormInfo | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<FormFillMessage | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const choose = async () => {
    setError(null);
    setSaved(null);
    setBusy(true);
    try {
      const next = await formFill.choose();
      if (next) {
        setForm(next);
        setValues({});
      }
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      setBusy(false);
    }
  };

  // After the file changed on disk: list its fields again but keep what the user typed for
  // fields that still exist (blank numbers may now point elsewhere, so the user re-checks).
  const reload = async () => {
    if (!form) return;
    setError(null);
    setBusy(true);
    try {
      const next = await formFill.reload(form.path);
      setForm(next);
      setValues((current) =>
        Object.fromEntries(
          Object.entries(current).filter(([key]) => next.fields.some((f) => f.key === key)),
        ),
      );
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!form) return;
    setError(null);
    setSaved(null);
    setBusy(true);
    try {
      setSaved(await formFill.fill(form, values));
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      setBusy(false);
    }
  };

  // Shown as "Blank 3" rather than the internal key "blank:3".
  const label = (key: string) =>
    key.startsWith('blank:') ? t('blank', { n: key.slice('blank:'.length) }) : key;
  const fileName = form?.path.split(/[\\/]/).pop();

  return (
    <section className="form-fill" aria-labelledby={`${id}-title`}>
      <header className="form-fill-head">
        {/* The mode switch above already says "fill a form"; the heading stays for screen readers. */}
        <h3 id={`${id}-title`} className="form-fill-sr-only">
          {t('title')}
        </h3>
        {/* The open form's name sits on the left, the button to switch forms on the right. */}
        {form ? (
          <p className="form-fill-file">
            <strong>{fileName}</strong>
            <span>{t('fieldCount', { count: form.fields.length })}</span>
          </p>
        ) : (
          <span />
        )}
        <button type="button" className="text-button" onClick={choose} disabled={busy}>
          <FileInput size={14} aria-hidden="true" />
          {form ? t('chooseOther') : t('choose')}
        </button>
      </header>
      {!form ? <p className="form-fill-note">{t('intro')}</p> : null}

      {form ? (
        <>
          {form.fields.length === 0 ? (
            <p className="form-fill-note">{t('noFields')}</p>
          ) : (
            <form
              className="form-fill-fields"
              onSubmit={(event) => {
                event.preventDefault();
                void save();
              }}
            >
              {form.fields.map((field) => (
                <label key={field.key} className="form-fill-field">
                  <span className="form-fill-key">{label(field.key)}</span>
                  {/* Where the field sits in the form, so blanks can be told apart. */}
                  <span className="form-fill-context">{field.context}</span>
                  <textarea
                    rows={1}
                    maxLength={maxValueLength}
                    value={values[field.key] ?? ''}
                    onChange={(event) =>
                      setValues((current) => ({ ...current, [field.key]: event.target.value }))
                    }
                  />
                </label>
              ))}
              <div className="form-fill-actions">
                <span className="form-fill-note">{t('emptyKept')}</span>
                <button type="submit" className="button primary" disabled={busy}>
                  <Save size={14} aria-hidden="true" />
                  {busy ? t('saving') : t('save')}
                </button>
              </div>
            </form>
          )}
        </>
      ) : null}

      <p className="form-fill-status" role="status">
        {saved ? t('saved', { path: saved }) : null}
      </p>
      {error ? (
        <p className="form-fill-error" role="alert">
          {t(error)}
          {error === 'changed' ? (
            <button type="button" className="text-button" onClick={reload} disabled={busy}>
              {t('reload')}
            </button>
          ) : null}
        </p>
      ) : null}
    </section>
  );
}
