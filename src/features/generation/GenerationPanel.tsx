import { useEffect, useId, useRef, useState } from 'react';
import { KeyRound, RefreshCw, Settings2 } from 'lucide-react';
import type { Language } from '../../lib/types';
import { generationText } from './messages';
import './generation.css';

import type {
  CredentialBackend,
  GenerationSlot,
  GenerationConfig,
  GenerationPreferences,
} from '../../lib/generation';
export type { GenerationSlot, GenerationConfig, GenerationPreferences } from '../../lib/generation';
export interface GenerationCredentialStatus {
  hasKey: boolean;
  support: CredentialBackend;
}
export interface GenerationCredentials {
  status: (slot: GenerationSlot, endpoint: string) => Promise<GenerationCredentialStatus>;
  set: (slot: GenerationSlot, endpoint: string, key: string) => Promise<void>;
  clear: (slot: GenerationSlot, endpoint: string) => Promise<void>;
}
export interface GenerationPanelProps {
  preferences: GenerationPreferences;
  onChange: (next: GenerationPreferences) => void;
  language: Language;
  native: boolean;
  credentials: GenerationCredentials;
}

function validEndpoint(endpoint: string, slot: GenerationSlot) {
  if (endpoint.length > 2048 || /[\u0000-\u0020\u007f\\]/u.test(endpoint)) return false;
  try {
    const url = new URL(endpoint);
    const authority = endpoint.split('://')[1]?.split('/')[0] ?? '';
    const rawHost = authority.startsWith('[')
      ? authority.slice(0, authority.indexOf(']') + 1)
      : authority.split(':')[0];
    const exactLocal = ['localhost', '127.0.0.1', '[::1]'].includes(rawHost.toLowerCase());
    const normalizedLocal = ['localhost', '127.0.0.1', '[::1]'].includes(
      url.hostname.toLowerCase(),
    );
    if (
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      authority.includes('@') ||
      url.port === '0'
    )
      return false;
    return slot === 'local'
      ? exactLocal && normalizedLocal && ['http:', 'https:'].includes(url.protocol)
      : !normalizedLocal && url.protocol === 'https:';
  } catch {
    return false;
  }
}

function NumberField({
  label,
  value,
  onChange,
  min,
  max,
  nullable = false,
  integer = true,
  help,
  language,
}: {
  label: string;
  value: number | null;
  onChange: (value: number | null) => void;
  min: number;
  max: number;
  nullable?: boolean;
  integer?: boolean;
  help?: string;
  language: Language;
}) {
  const id = useId();
  const [raw, setRaw] = useState(value === null ? '' : String(value));
  useEffect(() => setRaw(value === null ? '' : String(value)), [value]);
  const parsed = Number(raw);
  const valid =
    raw === ''
      ? nullable
      : Number.isFinite(parsed) &&
        parsed >= min &&
        parsed <= max &&
        (!integer || Number.isInteger(parsed));
  const error = valid
    ? ''
    : generationText(language, 'numberInvalid', {
        min,
        max,
        kind: generationText(language, integer ? 'integer' : 'number'),
      });
  return (
    <div className="generation-field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        type="number"
        value={raw}
        min={min}
        max={max}
        step={integer ? 1 : 'any'}
        required={!nullable}
        ref={(input) => input?.setCustomValidity(error)}
        aria-invalid={!valid}
        aria-describedby={`${id}-help ${id}-error`}
        onChange={(event) => {
          const next = event.target.value;
          setRaw(next);
          const number = event.target.valueAsNumber;
          if (next === '' && nullable) onChange(null);
          else if (
            Number.isFinite(number) &&
            number >= min &&
            number <= max &&
            (!integer || Number.isInteger(number))
          )
            onChange(number);
        }}
      />
      <small id={`${id}-help`} className="generation-note">
        {help}
      </small>
      <small id={`${id}-error`} className="generation-error">
        {error}
      </small>
    </div>
  );
}

function CredentialEditor({
  slot,
  endpoint,
  language,
  native,
  credentials,
}: {
  slot: GenerationSlot;
  endpoint: string;
  language: Language;
  native: boolean;
  credentials: GenerationCredentials;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const alive = useRef(false);
  const request = useRef(0);
  const pending = useRef(false);
  const [status, setStatus] = useState<GenerationCredentialStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<'keyError' | 'keyInvalid' | null>(null);
  const [message, setMessage] = useState<'stored' | 'cleared' | null>(null);
  const [revision, setRevision] = useState(0);
  const statusRequest = credentials.status;
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      request.current++;
      if (input.current) input.current.value = '';
    };
  }, []);
  useEffect(() => {
    if (!native || pending.current) return;
    let active = true;
    const current = ++request.current;
    setLoading(true);
    setError(null);
    void statusRequest(slot, endpoint)
      .then((next) => {
        if (active && alive.current && current === request.current) setStatus(next);
      })
      .catch(() => {
        // Provider exceptions may contain URLs or credentials. Never render or log them.
        if (active && alive.current && current === request.current) {
          setStatus(null);
          setError('keyError');
        }
      })
      .finally(() => {
        if (active && alive.current && current === request.current) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [slot, endpoint, native, revision, statusRequest]);
  const mutate = async (action: 'set' | 'clear') => {
    if (pending.current || !native || loading || !status || status.support === 'unsupported')
      return;
    let secret = action === 'set' ? (input.current?.value ?? '') : '';
    if (input.current) input.current.value = '';
    if (action === 'set' && !/^[\x21-\x7e]{1,8192}$/u.test(secret)) {
      setError('keyInvalid');
      return;
    }
    pending.current = true;
    const current = ++request.current;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      if (action === 'set') await credentials.set(slot, endpoint, secret);
      else await credentials.clear(slot, endpoint);
      if (alive.current && request.current === current) {
        setStatus((previous) => (previous ? { ...previous, hasKey: action === 'set' } : previous));
        setMessage(action === 'set' ? 'stored' : 'cleared');
      }
    } catch {
      if (alive.current && request.current === current) setError('keyError');
    } finally {
      secret = '';
      pending.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const disabled = busy || loading || !native || !status || status.support === 'unsupported';
  return (
    <div
      className="generation-credentials"
      aria-labelledby={`${id}-title`}
      aria-busy={busy || (native && loading)}
    >
      <h5 id={`${id}-title`}>
        <KeyRound size={15} />
        {generationText(language, 'credentials')}
      </h5>
      <p className="generation-note">{generationText(language, 'binding')}</p>
      <code className="generation-key-endpoint">{endpoint}</code>
      {!native ? (
        <p role="status" className="generation-note">
          {generationText(language, 'nativeOnly')}
        </p>
      ) : (
        <>
          <p role="status" aria-live="polite" className="generation-key-status">
            {generationText(
              language,
              loading
                ? 'checking'
                : status?.support === 'unsupported'
                  ? 'unsupported'
                  : status
                    ? status.hasKey
                      ? 'keyPresent'
                      : 'keyAbsent'
                    : 'keyError',
            )}
          </p>
          {status && status.support !== 'unsupported' ? (
            <p className="generation-note">{generationText(language, status.support)}</p>
          ) : null}
        </>
      )}
      <label htmlFor={`${id}-key`}>{generationText(language, 'keyInput')}</label>
      <input
        ref={input}
        id={`${id}-key`}
        type="password"
        autoComplete="new-password"
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        maxLength={8192}
        disabled={disabled}
        aria-describedby={`${id}-title`}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.preventDefault();
        }}
      />
      <div className="generation-actions">
        <button
          type="button"
          className="button"
          disabled={disabled}
          onClick={() => void mutate('set')}
        >
          {generationText(language, 'saveKey')}
        </button>
        <button
          type="button"
          className="button"
          disabled={disabled || !status?.hasKey}
          onClick={() => void mutate('clear')}
        >
          {generationText(language, 'clearKey')}
        </button>
        <button
          type="button"
          className="text-button"
          disabled={!native || busy || loading}
          onClick={() => {
            setMessage(null);
            setRevision((v) => v + 1);
          }}
        >
          <RefreshCw size={14} />
          {generationText(language, 'refresh')}
        </button>
      </div>
      {busy ? (
        <p role="status" className="generation-note">
          {generationText(language, 'working')}
        </p>
      ) : null}
      {message ? (
        <p role="status" className="generation-note">
          {generationText(language, message)}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="generation-error">
          {generationText(language, error)}
        </p>
      ) : null}
    </div>
  );
}

function defaultConfig(slot: GenerationSlot): GenerationConfig {
  return {
    endpoint: slot === 'local' ? 'http://127.0.0.1:11434/v1' : 'https://api.openai.com/v1',
    model: '',
    maxTokens: 2048,
    tokenParameter: slot === 'local' ? 'maxTokens' : 'maxCompletionTokens',
    temperature: null,
  };
}
function ProviderSettings({
  slot,
  config,
  onChange,
  language,
  native,
  credentials,
}: {
  slot: GenerationSlot;
  config: GenerationConfig | null;
  onChange: (config: GenerationConfig | null) => void;
  language: Language;
  native: boolean;
  credentials: GenerationCredentials;
}) {
  const id = useId();
  const t = (key: Parameters<typeof generationText>[1]) => generationText(language, key);
  const endpointValid = config ? validEndpoint(config.endpoint, slot) : false;
  const modelValid = Boolean(
    config?.model.trim() &&
    new TextEncoder().encode(config.model).length <= 256 &&
    !/[\u0000-\u001f\u007f]/u.test(config.model),
  );
  const update = (patch: Partial<GenerationConfig>) => {
    if (config) onChange({ ...config, ...patch });
  };
  return (
    <fieldset className="generation-provider">
      <legend>{t(slot)}</legend>
      <label className="generation-check" htmlFor={`${id}-enabled`}>
        <input
          id={`${id}-enabled`}
          type="checkbox"
          checked={config !== null}
          onChange={(event) => onChange(event.target.checked ? defaultConfig(slot) : null)}
        />
        <span>{t('enabled')}</span>
      </label>
      {config ? (
        <>
          <div className="generation-presets" role="group" aria-label={t('preset')}>
            {(slot === 'local' ? (['Ollama', 'LM Studio'] as const) : (['OpenAI'] as const)).map(
              (preset) => (
                <button
                  key={preset}
                  type="button"
                  className="button"
                  onClick={() =>
                    onChange({
                      ...config,
                      endpoint:
                        preset === 'Ollama'
                          ? 'http://127.0.0.1:11434/v1'
                          : preset === 'LM Studio'
                            ? 'http://127.0.0.1:1234/v1'
                            : 'https://api.openai.com/v1',
                      tokenParameter: preset === 'OpenAI' ? 'maxCompletionTokens' : 'maxTokens',
                      temperature: null,
                    })
                  }
                >
                  {preset}
                </button>
              ),
            )}
          </div>
          <p className="generation-note">{t('presetHelp')}</p>
          <div className="generation-field">
            <label htmlFor={`${id}-endpoint`}>{t('endpoint')}</label>
            <input
              id={`${id}-endpoint`}
              type="url"
              value={config.endpoint}
              maxLength={2048}
              required
              autoComplete="off"
              spellCheck={false}
              aria-invalid={!endpointValid}
              aria-describedby={`${id}-endpoint-help ${id}-endpoint-error`}
              ref={(input) => input?.setCustomValidity(endpointValid ? '' : t('endpointInvalid'))}
              onChange={(event) => update({ endpoint: event.target.value })}
            />
            <small id={`${id}-endpoint-help`} className="generation-note">
              {t('endpointHelp')}
            </small>
            <small id={`${id}-endpoint-error`} className="generation-error">
              {endpointValid ? '' : t('endpointInvalid')}
            </small>
          </div>
          <div className="generation-field">
            <label htmlFor={`${id}-model`}>{t('model')}</label>
            <input
              id={`${id}-model`}
              value={config.model}
              required
              maxLength={256}
              ref={(input) => input?.setCustomValidity(modelValid ? '' : t('modelInvalid'))}
              aria-invalid={!modelValid}
              aria-describedby={`${id}-model-error`}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => update({ model: event.target.value })}
            />
            <small id={`${id}-model-error`} className="generation-error">
              {modelValid ? '' : t('modelInvalid')}
            </small>
          </div>
          <div className="generation-grid">
            <NumberField
              label={t('maxTokens')}
              value={config.maxTokens}
              min={1}
              max={65536}
              language={language}
              onChange={(value) => {
                if (value !== null) update({ maxTokens: value });
              }}
            />
            <NumberField
              label={t('temperature')}
              value={config.temperature}
              min={0}
              max={2}
              nullable
              integer={false}
              language={language}
              help={t('temperatureHelp')}
              onChange={(value) => update({ temperature: value })}
            />
          </div>
          <div className="generation-field">
            <label htmlFor={`${id}-tokens`}>{t('tokenParameter')}</label>
            <select
              id={`${id}-tokens`}
              value={config.tokenParameter}
              onChange={(event) =>
                update({ tokenParameter: event.target.value as GenerationConfig['tokenParameter'] })
              }
            >
              <option value="maxTokens">max_tokens</option>
              <option value="maxCompletionTokens">max_completion_tokens</option>
            </select>
            <small className="generation-note">{t('tokenHelp')}</small>
          </div>
          <p className="generation-note">{t('disabledHelp')}</p>
          {endpointValid ? (
            <CredentialEditor
              key={`${slot}:${config.endpoint}`}
              slot={slot}
              endpoint={config.endpoint}
              language={language}
              native={native}
              credentials={credentials}
            />
          ) : null}
        </>
      ) : null}
    </fieldset>
  );
}

/** Can be nested in SettingsDialog's form; no form or implicit submit buttons. */
export function GenerationPanel({
  preferences,
  onChange,
  language,
  native,
  credentials,
}: GenerationPanelProps) {
  const id = useId();
  const t = (key: Parameters<typeof generationText>[1]) => generationText(language, key);
  const update = (patch: Partial<GenerationPreferences>) => onChange({ ...preferences, ...patch });
  return (
    <section className="generation-panel" aria-labelledby={`${id}-title`}>
      <header className="generation-heading">
        <Settings2 size={20} />
        <h3 id={`${id}-title`}>{t('title')}</h3>
      </header>
      <p className="generation-description">{t('description')}</p>
      <p className="generation-note">{t('draft')}</p>
      <div className="generation-grid">
        <div className="generation-field">
          <label htmlFor={`${id}-mode`}>{t('mode')}</label>
          <select
            id={`${id}-mode`}
            value={preferences.mode}
            onChange={(event) =>
              update({ mode: event.target.value as GenerationPreferences['mode'] })
            }
          >
            <option value="local">{t('local')}</option>
            <option value="external">{t('external')}</option>
            <option value="auto">{t('auto')}</option>
          </select>
        </div>
        <div className="generation-field">
          <label htmlFor={`${id}-priority`}>{t('priority')}</label>
          <select
            id={`${id}-priority`}
            value={preferences.priority}
            disabled={preferences.mode !== 'auto'}
            onChange={(event) =>
              update({ priority: event.target.value as GenerationPreferences['priority'] })
            }
          >
            <option value="localFirst">{t('localFirst')}</option>
            <option value="externalFirst">{t('externalFirst')}</option>
          </select>
        </div>
      </div>
      <div className="generation-field">
        <label htmlFor={`${id}-retrieval`}>{t('retrieval')}</label>
        <select
          id={`${id}-retrieval`}
          value={preferences.retrieval}
          aria-describedby={`${id}-retrieval-help`}
          onChange={(event) =>
            update({ retrieval: event.target.value as GenerationPreferences['retrieval'] })
          }
        >
          <option value="hybrid">{t('retrievalHybrid')}</option>
          <option value="semantic">{t('retrievalSemantic')}</option>
          <option value="keyword">{t('retrievalKeyword')}</option>
        </select>
        <small id={`${id}-retrieval-help`} className="generation-note">
          {t('retrievalHelp')}
        </small>
      </div>
      <label className="generation-check" htmlFor={`${id}-fallback`}>
        <input
          id={`${id}-fallback`}
          type="checkbox"
          checked={preferences.fallback}
          aria-describedby={`${id}-fallback-help`}
          onChange={(event) => update({ fallback: event.target.checked })}
        />
        <span>{t('fallback')}</span>
      </label>
      <p id={`${id}-fallback-help`} className="generation-note">
        {t('fallbackHelp')}
      </p>
      <label className="generation-check" htmlFor={`${id}-questions`}>
        <input
          id={`${id}-questions`}
          type="checkbox"
          checked={preferences.allowExternalQuestions}
          aria-describedby={`${id}-permission-help`}
          onChange={(event) => update({ allowExternalQuestions: event.target.checked })}
        />
        <span>{t('allowQuestions')}</span>
      </label>
      <p id={`${id}-permission-help`} className="generation-permission">
        {t('permissionHelp')}
      </p>
      <div className="generation-grid">
        <NumberField
          label={t('context')}
          value={preferences.maxContextChars}
          min={512}
          max={200000}
          nullable
          language={language}
          help={generationText(language, 'defaultNumber', { value: 8000, min: 512, max: 200000 })}
          onChange={(value) => update({ maxContextChars: value })}
        />
        <NumberField
          label={t('topK')}
          value={preferences.topK}
          min={1}
          max={32}
          nullable
          language={language}
          help={generationText(language, 'defaultNumber', { value: 6, min: 1, max: 32 })}
          onChange={(value) => update({ topK: value })}
        />
      </div>
      <ProviderSettings
        slot="local"
        config={preferences.local}
        onChange={(local) => update({ local })}
        language={language}
        native={native}
        credentials={credentials}
      />
      <ProviderSettings
        slot="external"
        config={preferences.external}
        onChange={(external) => update({ external })}
        language={language}
        native={native}
        credentials={credentials}
      />
    </section>
  );
}
