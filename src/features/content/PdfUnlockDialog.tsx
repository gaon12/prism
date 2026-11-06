import { useEffect, useId, useRef, useState } from 'react';
import { LockKeyhole, X } from 'lucide-react';
import { content } from '../../lib/content';
import type { Language } from '../../lib/types';
import { showToast } from '../../lib/store';
import { contentText } from './messages';
import './pdf-unlock.css';

export function PdfUnlockDialog({
  path,
  language,
  onClose,
}: {
  path: string;
  language: Language;
  onClose: () => void;
}) {
  const id = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const alive = useRef(false);
  const closeCallback = useRef(onClose);
  closeCallback.current = onClose;
  const active = useRef<{ token: string | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Parameters<typeof contentText>[1] | null>(null);
  const t = (key: Parameters<typeof contentText>[1]) => contentText(language, key);
  const office = /\.(docx|xlsx|pptx)$/iu.test(path);
  const clear = () => {
    if (input.current) input.current.value = '';
    const token = active.current?.token;
    active.current = null;
    if (token) void content.cancelUnlock(token).catch(() => {});
  };
  const close = () => {
    clear();
    closeCallback.current();
  };
  useEffect(() => {
    alive.current = true;
    const previous = document.activeElement;
    const element = dialog.current;
    element?.showModal();
    input.current?.focus();
    return () => {
      alive.current = false;
      clear();
      element?.close();
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
      else document.querySelector<HTMLInputElement>('input[aria-autocomplete="list"]')?.focus();
    };
  }, []);
  const submit = async () => {
    if (active.current || !input.current?.reportValidity()) return;
    const password = input.current.value;
    input.current.value = '';
    if (new TextEncoder().encode(password).length > 1024) {
      setError('unlockLong');
      return;
    }
    const attempt = { token: null as string | null };
    active.current = attempt;
    setBusy(true);
    setError(null);
    try {
      // No password is sent until the server has registered this token. If the
      // user closes the dialog during that handshake, discard the late token.
      const token = await content.prepareUnlock();
      if (!alive.current || active.current !== attempt) {
        void content.cancelUnlock(token).catch(() => {});
        return;
      }
      attempt.token = token;
      await content.unlock(path, password, token);
      if (!alive.current || active.current !== attempt) return;
      active.current = null;
      showToast(t(office ? 'unlockOfficeDone' : 'unlockDone'));
      closeCallback.current();
    } catch (cause) {
      if (!alive.current || active.current !== attempt) return;
      const code =
        cause && typeof cause === 'object' && 'code' in cause
          ? String(cause.code).toLowerCase()
          : '';
      const known = {
        passwordrejected: office ? 'unlockOfficeRejected' : 'unlockRejected',
        busy: office ? 'unlockOfficeBusy' : 'unlockBusy',
        unsupportedformat: office ? 'unlockOfficeUnsupported' : 'unlockFailed',
        passwordtimedout: 'unlockTimedOut',
        cancelled: 'cancelled',
        outsidescope: 'outsideScope',
        unsafepath: 'unsafePath',
        sourcechanged: 'sourceChanged',
        ocrrequired: 'ocr',
        limitexceeded: 'budget',
        corruptdocument: 'corrupt',
        io: 'io',
      } as const;
      setError(
        known[code as keyof typeof known] ?? (office ? 'unlockOfficeFailed' : 'unlockFailed'),
      );
      input.current?.focus();
    } finally {
      if (attempt.token) void content.cancelUnlock(attempt.token).catch(() => {});
      if (alive.current && active.current === attempt) {
        active.current = null;
        setBusy(false);
      }
    }
  };
  return (
    <dialog
      ref={dialog}
      className="pdf-unlock-dialog"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-notice`}
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
      onKeyDown={(event) => event.stopPropagation()}
    >
      <form
        aria-busy={busy}
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
        autoComplete="off"
      >
        <header>
          <LockKeyhole size={21} aria-hidden="true" />
          <h2 id={`${id}-title`}>{t(office ? 'unlockOfficeTitle' : 'unlockTitle')}</h2>
          <button
            type="button"
            className="icon-button"
            aria-label={t('unlockClose')}
            onClick={close}
          >
            <X size={19} />
          </button>
        </header>
        <p className="pdf-unlock-name" title={path}>
          {path.split(/[\\/]/).pop()}
        </p>
        <p id={`${id}-notice`} className="pdf-unlock-notice">
          {t(office ? 'unlockOfficeNotice' : 'unlockNotice')}
        </p>
        <label htmlFor={`${id}-password`}>
          {t(office ? 'unlockOfficePassword' : 'unlockPassword')}
        </label>
        <input
          ref={input}
          id={`${id}-password`}
          type="password"
          required
          maxLength={1024}
          autoComplete="off"
          spellCheck={false}
          aria-describedby={error ? `${id}-error` : undefined}
          aria-invalid={Boolean(error)}
          readOnly={busy}
        />
        {error ? (
          <p id={`${id}-error`} className="pdf-unlock-error" role="alert">
            {t(error)}
          </p>
        ) : null}
        {busy ? (
          <p className="pdf-unlock-notice" role="status">
            {t(office ? 'unlockOfficeWorking' : 'unlockWorking')}
          </p>
        ) : null}
        <footer>
          <button type="button" className="button" onClick={close}>
            {t('unlockCancel')}
          </button>
          <button type="submit" className="button primary" disabled={busy}>
            {t('unlockSubmit')}
          </button>
        </footer>
      </form>
    </dialog>
  );
}
