import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ClientSettings, Language } from '../../lib/types';
import { ocrText } from './ocr-messages';

interface Capabilities {
  engine: string;
  available: boolean;
  pdfAvailable: boolean;
  languages: { tag: string; name: string }[];
  reason: string | null;
}

export function OcrSettings({
  value,
  onChange,
  language,
  native,
}: {
  value: ClientSettings['ocr'];
  onChange: (value: ClientSettings['ocr']) => void;
  language: Language;
  native: boolean;
}) {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{ capabilities?: Capabilities; failed?: boolean }>({});
  const t = (key: Parameters<typeof ocrText>[1]) => ocrText(language, key);
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    setState({});
    void invoke<Capabilities>('content_ocr_capabilities').then(
      (capabilities) => {
        if (!disposed) setState({ capabilities });
      },
      () => {
        if (!disposed) setState({ failed: true });
      },
    );
    return () => {
      disposed = true;
    };
  }, [native, attempt]);
  const capabilities = state.capabilities;
  const selectedInstalled =
    value.language === 'auto' ||
    capabilities?.languages.some((l) => {
      const tag = l.tag.toLowerCase();
      const selected = value.language.toLowerCase();
      return tag === selected || tag.split('-')[0] === selected || selected.split('-')[0] === tag;
    });
  const engine =
    capabilities?.engine === 'windowsNative'
      ? t('windowsNative')
      : capabilities?.engine === 'macNative'
        ? t('macNative')
        : capabilities?.engine === 'tesseract'
          ? t('tesseract')
          : null;
  return (
    <section aria-labelledby="ocr-settings-title">
      <h3 id="ocr-settings-title">{t('title')}</h3>
      <label className="setting-check">
        <span>{t('enabled')}</span>
        <input
          type="checkbox"
          checked={value.enabled}
          onChange={(event) => onChange({ ...value, enabled: event.target.checked })}
        />
      </label>
      <p className="setting-footnote">{t('privacy')}</p>
      <div className="setting-field">
        <label htmlFor="ocr-language">{t('language')}</label>
        <select
          id="ocr-language"
          value={value.language}
          disabled={!native || !capabilities?.available}
          onChange={(event) => onChange({ ...value, language: event.target.value })}
        >
          <option value="auto">{t('auto')}</option>
          {capabilities?.languages.map((item) => (
            <option key={item.tag} value={item.tag}>
              {item.name} · {item.tag}
            </option>
          ))}
          {value.language !== 'auto' &&
          !capabilities?.languages.some((item) => item.tag === value.language) ? (
            <option value={value.language}>{value.language}</option>
          ) : null}
        </select>
      </div>
      <p className="setting-footnote" role="status">
        {!native
          ? t('browser')
          : state.failed
            ? t('checkFailed')
            : !capabilities
              ? t('checking')
              : !capabilities.available
                ? t(
                    capabilities.reason === 'languageUnavailable'
                      ? 'missingLanguage'
                      : 'unavailable',
                  )
                : !selectedInstalled
                  ? t('missingLanguage')
                  : engine}
      </p>
      {capabilities?.available && !capabilities.pdfAvailable ? (
        <p className="setting-footnote">{t('pdfUnavailable')}</p>
      ) : null}
      {native ? (
        <button
          type="button"
          className="text-button"
          disabled={!state.failed && !capabilities}
          onClick={() => setAttempt((n) => n + 1)}
        >
          {t('refresh')}
        </button>
      ) : null}
    </section>
  );
}
