// "Fill an existing form" panel (requirement Q46).
//
// 1. Open a DOCX/HWPX form → the native side lists its fields.
// 2. Type a value for each field (placeholders with the same name are filled together).
// 3. Save → a new file is written; the original form is never modified.
import { useEffect, useId, useRef, useState } from 'react';
import { FileInput, Save } from 'lucide-react';
import { formFill, maxValueLength, type FormError, type FormInfo } from '../../lib/form-fill';
import type { Language } from '../../lib/types';
import { formFillText, type FormFillMessage } from './form-fill-messages';
import './form-fill.css';

type PreviousInput = { key: string; context: string; value: string };
const inputValue = (values: Record<string, string>, key: string) =>
  Object.hasOwn(values, key) ? values[key] : '';

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

export function FormFillPanel({
  language,
  onDirtyChange,
}: {
  language: Language;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const t = (key: FormFillMessage, values?: Record<string, string | number>) =>
    formFillText(language, key, values);
  const id = useId();
  const [form, setForm] = useState<FormInfo | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [savedValues, setSavedValues] = useState<Record<string, string>>({});
  const [previousInputs, setPreviousInputs] = useState<PreviousInput[] | null>(null);
  const [operation, setOperation] = useState<'choose' | 'reload' | 'save' | null>(null);
  const pending = useRef(false);
  const busy = operation !== null;
  const dirty = Boolean(
    previousInputs ||
    form?.fields.some(
      (field) => inputValue(values, field.key) !== inputValue(savedValues, field.key),
    ),
  );
  useEffect(() => {
    onDirtyChange?.(dirty || busy);
    return () => onDirtyChange?.(false);
  }, [dirty, busy, onDirtyChange]);
  const [error, setError] = useState<FormFillMessage | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const begin = (next: Exclude<typeof operation, null>) => {
    if (pending.current) return false;
    pending.current = true;
    setOperation(next);
    return true;
  };
  const finish = () => {
    pending.current = false;
    setOperation(null);
  };

  const choose = async () => {
    if (dirty || !begin('choose')) return;
    setError(null);
    setSaved(null);
    try {
      const next = await formFill.choose();
      if (next) {
        setForm(next);
        setValues({});
        setSavedValues({});
        setPreviousInputs(null);
      }
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      finish();
    }
  };

  // Named placeholders can keep their values. Positional blank numbers are not
  // identities after a fingerprint change; retain their old context for review.
  const reload = async () => {
    if (!form || previousInputs || !begin('reload')) return;
    setError(null);
    try {
      const next = await formFill.reload(form.path);
      setForm(next);
      if (next.fingerprint !== form.fingerprint) {
        const named = new Set(
          next.fields.filter((field) => field.kind === 'placeholder').map((field) => field.key),
        );
        const kept: [string, string][] = [];
        const review: PreviousInput[] = [];
        for (const field of form.fields) {
          const value = inputValue(values, field.key);
          if (field.kind === 'placeholder' && named.has(field.key)) kept.push([field.key, value]);
          else if (value) review.push({ key: field.key, context: field.context, value });
        }
        setValues(Object.fromEntries(kept));
        setSavedValues({});
        setPreviousInputs(review.length ? review : null);
        setSaved(null);
      }
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      finish();
    }
  };

  const save = async () => {
    if (!form || previousInputs || !begin('save')) return;
    setError(null);
    setSaved(null);
    try {
      // The editor may change while the native save is in flight. Only this
      // immutable input snapshot becomes clean when that save actually succeeds.
      const submitted = { ...values };
      const path = await formFill.fill(form, submitted);
      setSaved(path);
      if (path) setSavedValues(submitted);
    } catch (failure) {
      setError(errorKey(failure));
    } finally {
      finish();
    }
  };

  // Shown as "Blank 3" rather than the internal key "blank:3".
  const label = (key: string) =>
    key.startsWith('field:')
      ? key.slice('field:'.length)
      : key.startsWith('blank:')
        ? t('blank', { n: key.slice('blank:'.length) })
        : key;
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
        <button type="button" className="text-button" onClick={choose} disabled={busy || dirty}>
          <FileInput size={14} aria-hidden="true" />
          {form ? t('chooseOther') : t('choose')}
        </button>
      </header>
      {!form ? <p className="form-fill-note">{t('intro')}</p> : null}
      {dirty ? (
        <div className="form-fill-review" role="status">
          <p>{t('unsaved')}</p>
          <button
            type="button"
            className="text-button"
            disabled={busy}
            onClick={() => {
              setValues({ ...savedValues });
              setPreviousInputs(null);
              setError(null);
            }}
          >
            {t('discard')}
          </button>
        </div>
      ) : null}
      {previousInputs ? (
        <section className="form-fill-review" aria-label={t('previousInputs')}>
          <h4>{t('previousInputs')}</h4>
          <p className="form-fill-note">{t('previousHelp')}</p>
          {previousInputs.map((field) => (
            <label key={field.key} className="form-fill-field">
              <span className="form-fill-key">{label(field.key)}</span>
              <span className="form-fill-context">{field.context}</span>
              <textarea rows={2} value={field.value} readOnly />
            </label>
          ))}
          <button type="button" className="button" onClick={() => setPreviousInputs(null)}>
            {t('reviewPrevious')}
          </button>
        </section>
      ) : null}

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
                    value={inputValue(values, field.key)}
                    disabled={busy && operation !== 'save'}
                    onChange={(event) =>
                      setValues((current) => ({ ...current, [field.key]: event.target.value }))
                    }
                  />
                </label>
              ))}
              <div className="form-fill-actions">
                <span className="form-fill-note">{t('emptyKept')}</span>
                <button
                  type="submit"
                  className="button primary"
                  disabled={busy || Boolean(previousInputs)}
                >
                  <Save size={14} aria-hidden="true" />
                  {operation === 'save' ? t('saving') : t('save')}
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
            <button
              type="button"
              className="text-button"
              onClick={reload}
              disabled={busy || Boolean(previousInputs)}
            >
              {t('reload')}
            </button>
          ) : null}
        </p>
      ) : null}
    </section>
  );
}
