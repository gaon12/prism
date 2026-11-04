import { useEffect, useState } from 'react';
import { Clipboard, Copy, Pin, Trash2, Pause, Play, Settings2 } from 'lucide-react';
import { clipboard } from '../../lib/clipboard';
import type { ClipboardController, ClipKind } from '../../lib/clipboard';
import { native } from '../../lib/bridge';
import { useClient, saveSettings, showToast, errorMessage } from '../../lib/store';
import { translate } from '../../lib/i18n';
import { clipText } from './messages';
import './clipboard.css';

export function ClipboardNotice() {
  const { settings } = useClient();
  const [busy, setBusy] = useState(false);
  const t = (key: Parameters<typeof clipText>[1]) => clipText(settings.language, key);
  if (!native || settings.clipboard.onboarded) return null;
  const acknowledge = async (enabled: boolean) => {
    setBusy(true);
    try {
      await saveSettings({
        ...settings,
        clipboard: { ...settings.clipboard, enabled, onboarded: true },
      });
    } catch (cause) {
      showToast(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <aside className="clipboard-notice" aria-label={t('title')}>
      <Clipboard size={21} />
      <div>
        <strong>{t('title')}</strong>
        <p>{t('notice')}</p>
        <small>{t('local')}</small>
      </div>
      <div className="clipboard-notice-actions">
        <button className="button primary" disabled={busy} onClick={() => void acknowledge(true)}>
          {t('keep')}
        </button>
        <button className="text-button" disabled={busy} onClick={() => void acknowledge(false)}>
          {t('disable')}
        </button>
      </div>
    </aside>
  );
}

function retentionInteger(value: string, min: number, max: number) {
  const number = Number(value);
  return value.trim() && Number.isSafeInteger(number) && number >= min && number <= max
    ? number
    : null;
}

function ClipboardSettings({ onClose }: { onClose: () => void }) {
  const { settings } = useClient();
  const [draft, setDraft] = useState(() => ({ ...settings.clipboard }));
  const [limits, setLimits] = useState(() => ({
    items: String(settings.clipboard.maxItems),
    days: String(settings.clipboard.maxAgeDays),
    mib: String(settings.clipboard.maxBytes / 1024 ** 2),
  }));
  const maxItems = retentionInteger(limits.items, 10, 10000);
  const maxAgeDays = retentionInteger(limits.days, 0, 3650);
  const maxMiB = retentionInteger(limits.mib, 16, 8192);
  const valid = maxItems !== null && maxAgeDays !== null && maxMiB !== null;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const t = (key: Parameters<typeof clipText>[1]) => clipText(settings.language, key);
  const submit = async () => {
    if (busy || maxItems === null || maxAgeDays === null || maxMiB === null) return;
    setBusy(true);
    setError(null);
    try {
      await saveSettings({
        ...settings,
        clipboard: {
          ...draft,
          maxItems,
          maxAgeDays,
          maxBytes: maxMiB * 1024 ** 2,
          onboarded: true,
        },
      });
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="clipboard-settings"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label>
        <input
          type="checkbox"
          checked={draft.enabled}
          onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
        />
        {t('enabled')}
      </label>
      <label>
        <input
          type="checkbox"
          checked={draft.captureImages}
          onChange={(event) => setDraft({ ...draft, captureImages: event.target.checked })}
        />
        {t('images')}
      </label>
      <label>
        {t('maxItems')}
        <input
          type="number"
          min={10}
          max={10000}
          step={1}
          required
          aria-invalid={maxItems === null}
          value={limits.items}
          onChange={(event) => setLimits({ ...limits, items: event.target.value })}
        />
      </label>
      <label>
        {t('days')}
        <input
          type="number"
          min={0}
          max={3650}
          step={1}
          required
          aria-invalid={maxAgeDays === null}
          value={limits.days}
          onChange={(event) => setLimits({ ...limits, days: event.target.value })}
        />
      </label>
      <label>
        {t('bytes')}
        <input
          type="number"
          min={16}
          max={8192}
          step={1}
          required
          aria-invalid={maxMiB === null}
          value={limits.mib}
          onChange={(event) => setLimits({ ...limits, mib: event.target.value })}
        />
      </label>
      <p className="muted">{t('pinnedHint')}</p>
      {!valid ? (
        <p className="inline-error" role="status">
          {t('invalidLimits')}
        </p>
      ) : null}
      {error ? (
        <p className="inline-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="clipboard-actions">
        <button className="button primary" disabled={busy || !valid}>
          {t('save')}
        </button>
        <button className="button" type="button" disabled={busy} onClick={onClose}>
          {t('cancel')}
        </button>
      </div>
    </form>
  );
}

export function ClipboardPanel({
  controller,
  compact = false,
}: {
  controller: ClipboardController;
  compact?: boolean;
}) {
  const { settings } = useClient();
  const t = (key: Parameters<typeof clipText>[1]) => clipText(settings.language, key);
  const { data, selected } = controller;
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState(false);
  const [options, setOptions] = useState(false);
  const [preview, setPreview] = useState<{ id: string; content: string } | null>(null);
  const [previewError, setPreviewError] = useState<{ id: string; message: string } | null>(null);
  const selectedId = selected?.id;
  const selectedKind = selected?.kind;
  useEffect(() => {
    if (!selectedId || !native) return;
    let cancelled = false;
    void (selectedKind === 'image' ? clipboard.thumbnail(selectedId) : clipboard.text(selectedId))
      .then((content) => {
        if (!cancelled) {
          setPreview({ id: selectedId, content });
          setPreviewError(null);
        }
      })
      .catch((cause) => {
        if (!cancelled) setPreviewError({ id: selectedId, message: errorMessage(cause) });
      });
    return () => {
      cancelled = true;
    };
  }, [selectedId, selectedKind]);
  const run = async (work: () => Promise<unknown>, copied = false) => {
    setBusy(true);
    try {
      await work();
      controller.refresh();
      if (copied) showToast(translate(settings.language, 'copied'));
    } catch (cause) {
      showToast(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };
  if (!native)
    return (
      <section className="clipboard-panel">
        <h2>{t('title')}</h2>
        <p>{translate(settings.language, 'nativeOnly')}</p>
      </section>
    );
  return (
    <section className={`clipboard-panel ${compact ? 'compact' : ''}`} aria-label={t('title')}>
      <header className="clipboard-heading">
        <Clipboard size={19} />
        <strong>{t('title')}</strong>
        <span className="muted">{data?.status.count ?? '—'}</span>
        <button
          className="icon-button small"
          title={t(data?.status.paused ? 'resume' : 'pause')}
          aria-label={t(data?.status.paused ? 'resume' : 'pause')}
          disabled={busy || !data?.status.enabled}
          onClick={() => void run(() => clipboard.pause(!data?.status.paused))}
        >
          {data?.status.paused ? <Play size={16} /> : <Pause size={16} />}
        </button>
        <button
          className="icon-button small"
          aria-label={t('settings')}
          aria-expanded={options}
          onClick={() => setOptions((value) => !value)}
        >
          <Settings2 size={17} />
        </button>
      </header>
      {options ? (
        <ClipboardSettings onClose={() => setOptions(false)} />
      ) : (
        <>
          <div className="clipboard-filters">
            {([null, 'text', 'link', 'image'] as (ClipKind | null)[]).map((kind) => (
              <button
                key={kind ?? 'all'}
                className={`mode ${controller.kind === kind ? 'active' : ''}`}
                aria-pressed={controller.kind === kind}
                onClick={() => controller.setKind(kind)}
              >
                {t(kind ?? 'all')}
              </button>
            ))}
            <button
              className="text-button"
              disabled={busy || !data?.status.unpinnedCount}
              onClick={() => setConfirmation(true)}
            >
              {t('clear')}
            </button>
          </div>
          {confirmation ? (
            <div className="clipboard-confirm" role="alert">
              <span>{t('confirm')}</span>
              <button
                className="button"
                disabled={busy || !data?.status.unpinnedCount}
                onClick={() =>
                  void run(async () => {
                    await clipboard.clear();
                    setConfirmation(false);
                  })
                }
              >
                {t('delete')}
              </button>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => setConfirmation(false)}
              >
                {t('cancel')}
              </button>
            </div>
          ) : null}
          {data?.status.paused || data?.status.enabled === false ? (
            <p className="clipboard-state" role="status">
              {t(data.status.paused ? 'paused' : 'disabled')}
            </p>
          ) : null}
          {controller.error || data?.status.lastError ? (
            <p className="inline-error" role="alert">
              {controller.error ?? data?.status.lastError}
            </p>
          ) : null}
          <div className="clipboard-body">
            <div
              className="clipboard-list"
              id={controller.listId}
              role="listbox"
              aria-label={t('title')}
            >
              {!data && !controller.error ? (
                <p role="status">{t('loading')}</p>
              ) : !data?.items.length ? (
                <p className="clipboard-empty">{t(data?.status.count ? 'noMatches' : 'empty')}</p>
              ) : (
                data.items.map((item) => (
                  <button
                    key={item.id}
                    id={`${controller.listId}-${item.id}`}
                    role="option"
                    aria-selected={selectedId === item.id}
                    className="clipboard-row"
                    onClick={() => controller.select(item.id)}
                    onDoubleClick={() => void run(() => clipboard.copy(item.id), true)}
                  >
                    <span className="clipboard-kind">
                      {item.pinned ? <Pin size={14} /> : t(item.kind)}
                    </span>
                    <span className="clipboard-snippet">{item.preview}</span>
                    <small>{new Date(item.lastCopiedMs).toLocaleString(settings.language)}</small>
                  </button>
                ))
              )}
            </div>
            {selected ? (
              <aside className="clipboard-detail" aria-label={t('preview')}>
                <div className="clipboard-actions">
                  <button
                    className="button primary"
                    disabled={busy}
                    onClick={() => void run(() => clipboard.copy(selected.id), true)}
                  >
                    <Copy size={15} />
                    {t('copy')}
                  </button>
                  <button
                    className="icon-button small"
                    aria-label={t(selected.pinned ? 'unpin' : 'pin')}
                    aria-pressed={selected.pinned}
                    disabled={busy}
                    onClick={() => void run(() => clipboard.pin(selected.id, !selected.pinned))}
                  >
                    <Pin size={16} />
                  </button>
                  <button
                    className="icon-button small"
                    aria-label={t('delete')}
                    disabled={busy}
                    onClick={() => void run(() => clipboard.delete(selected.id))}
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
                {preview?.id === selected.id ? (
                  selected.kind === 'image' ? (
                    <img
                      className="clipboard-image"
                      src={preview.content}
                      alt={`${selected.width} × ${selected.height}`}
                    />
                  ) : (
                    <pre className="clipboard-full-text">{preview.content}</pre>
                  )
                ) : previewError?.id === selected.id ? (
                  <p className="inline-error">{previewError.message}</p>
                ) : (
                  <p>{t('loading')}</p>
                )}
              </aside>
            ) : null}
          </div>
          {data && data.total > data.items.length ? <small>{t('limited')}</small> : null}
          <small className="clipboard-local">
            {t('local')} · {((data?.status.bytes ?? 0) / 1024 ** 2).toFixed(1)} MiB
          </small>
        </>
      )}
    </section>
  );
}
