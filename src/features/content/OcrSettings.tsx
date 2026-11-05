import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type { ClientSettings, Language } from '../../lib/types';
import { ocrText } from './ocr-messages';
import type { OcrRuntimeStatus } from './ocr-runtime';
import './ocr.css';

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
  const [model, setModel] = useState<OcrRuntimeStatus>();
  const [modelFailed, setModelFailed] = useState(false);
  const [modelAction, setModelAction] = useState(false);
  const modelEpoch = useRef(0);
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
  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let epoch = 0;
    let stop: (() => void) | undefined;
    void listen<OcrRuntimeStatus>('prism:ocr', (event) => {
      epoch++;
      modelEpoch.current++;
      if (!disposed) {
        setModel(event.payload);
        setModelFailed(false);
      }
    })
      .then(async (unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        stop = unlisten;
        const snapshot = epoch;
        const value = await invoke<OcrRuntimeStatus>('ocr_status');
        if (!disposed && epoch === snapshot) {
          setModel(value);
          setModelFailed(false);
        }
      })
      .catch(() => {
        if (!disposed) setModelFailed(true);
      });
    return () => {
      disposed = true;
      stop?.();
    };
  }, [native, attempt]);
  const installing = modelAction || model?.phase === 'installing';
  const install = async (offline: boolean) => {
    setModelFailed(false);
    try {
      const folder = offline ? await open({ directory: true, multiple: false }) : null;
      if (offline && !folder) return;
      setModelAction(true);
      await invoke<OcrRuntimeStatus>('ocr_install', {
        offlineDirectory: typeof folder === 'string' ? folder : null,
      });
    } catch (cause) {
      setModelFailed(
        !(cause && typeof cause === 'object' && 'code' in cause && cause.code === 'cancelled'),
      );
    } finally {
      setModelAction(false);
      const snapshot = modelEpoch.current;
      void invoke<OcrRuntimeStatus>('ocr_status')
        .then((status) => {
          if (modelEpoch.current === snapshot) setModel(status);
        })
        .catch(() => {});
    }
  };
  const size = (bytes: number) =>
    `${new Intl.NumberFormat(language, { maximumFractionDigits: 1 }).format(bytes / 1024 ** 3)} GiB`;
  const capabilities = state.capabilities;
  const selectedInstalled =
    value.language === 'auto' ||
    capabilities?.languages.some((l) => {
      const tag = l.tag.replaceAll('_', '-').toLowerCase();
      const selected = value.language.replaceAll('_', '-').toLowerCase();
      return (
        tag === selected ||
        selected.startsWith(`${tag}-`) ||
        tag.split('-')[0] === selected ||
        selected.split('-')[0] === tag
      );
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
        <label htmlFor="ocr-engine">{t('engine')}</label>
        <select
          id="ocr-engine"
          value={value.engine}
          disabled={!native}
          onChange={(event) =>
            onChange({ ...value, engine: event.target.value as ClientSettings['ocr']['engine'] })
          }
        >
          <option value="auto">{t('autoEngine')}</option>
          <option value="system">{t('systemEngine')}</option>
          <option value="paddleVl16">PaddleOCR-VL-1.6</option>
        </select>
      </div>
      <p className="setting-footnote">{t('engineHint')}</p>
      <div className="setting-field">
        <label htmlFor="ocr-language">{t('language')}</label>
        <select
          id="ocr-language"
          value={value.language}
          disabled={!native || !capabilities?.available || value.engine === 'paddleVl16'}
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
        {value.engine === 'paddleVl16'
          ? t('multilingual')
          : !native
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
      {value.engine !== 'paddleVl16' &&
      !model?.available &&
      capabilities?.available &&
      !capabilities.pdfAvailable ? (
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
      {native ? (
        <div className="ocr-model-card">
          <h4>PaddleOCR-VL-1.6</h4>
          <p className="setting-footnote" role="status">
            {modelFailed
              ? t('modelFailed')
              : !model
                ? t('modelChecking')
                : t(
                    (
                      {
                        unsupported: 'modelUnsupported',
                        notInstalled: 'modelNotInstalled',
                        installing: 'modelInstalling',
                        ready: 'modelReady',
                        starting: 'modelStarting',
                        running: 'modelRunning',
                        cancelled: 'modelCancelled',
                        error: 'modelFailed',
                      } as const
                    )[model.phase],
                  )}
          </p>
          <p className="setting-footnote">{t('modelHint')}</p>
          {model && model.totalBytes > 0 ? (
            <p className="setting-footnote">
              {t('downloadSize')}: {size(model.totalBytes)} · {t('diskSize')}:{' '}
              {size(model.expandedBytes)}
            </p>
          ) : null}
          {installing && model ? (
            <>
              <progress
                className="ocr-model-progress"
                aria-label={t('downloadSize')}
                value={model.downloadedBytes}
                max={model.totalBytes || 1}
              />
              <p className="setting-footnote">
                {size(model.downloadedBytes)} / {size(model.totalBytes)} · {t('installedFiles')}:{' '}
                {model.processedFiles.toLocaleString(language)} /{' '}
                {model.totalFiles.toLocaleString(language)}
              </p>
            </>
          ) : null}
          <div className="ocr-model-actions">
            {installing ? (
              <button
                type="button"
                className="button"
                onClick={() => void invoke('ocr_cancel_install').catch(() => setModelFailed(true))}
              >
                {t('cancelModel')}
              </button>
            ) : model && !model.available && model.phase !== 'unsupported' ? (
              <>
                <button type="button" className="button" onClick={() => void install(false)}>
                  {t('installModel')}
                </button>
                <button type="button" className="button" onClick={() => void install(true)}>
                  {t('offlineModel')}
                </button>
              </>
            ) : null}
          </div>
        </div>
      ) : null}
    </section>
  );
}
