// First-run tour: welcome → shortcut → folders → privacy → ready.
//
// The component only edits a local draft (see model.ts). It talks to the rest of the app through
// props, so the app decides when to show it and how settings are saved:
//
//   <Onboarding
//     settings={settings}
//     loadRecommended={() => bridge.recommendedFolders()}
//     pickFolders={() => bridge.chooseFolders()}
//     onFinish={async (next) => { await saveSettings(next); markOnboarded(); }}
//     onSkip={() => markOnboarded()}
//   />
//
// `onFinish` may throw (for example when the shortcut is taken by another app); the message is
// shown and the user stays on the last step to fix it.

import { useEffect, useMemo, useRef, useState } from 'react';
import { Check, FolderPlus } from 'lucide-react';
import { Brand } from '../../components/Brand';
import { languageNames } from '../../lib/i18n';
import type { ClientSettings, Language, RecommendedFolder, Theme } from '../../lib/types';
import { onboardingText } from './messages';
import type { OnboardingKey } from './messages';
import {
  addFolder,
  applyDraft,
  folderKey,
  initialDraft,
  shortcutFromKey,
  shortcutKeys,
  steps,
} from './model';
import type { FolderChoice, OnboardingDraft, StepId } from './model';
import './onboarding.css';

export interface OnboardingProps {
  settings: ClientSettings;
  /** Suggested folders (Documents, Desktop, ...). May reject; the list then starts empty. */
  loadRecommended: () => Promise<RecommendedFolder[]>;
  /** Opens the folder picker; resolves to the chosen paths (empty when cancelled). */
  pickFolders: () => Promise<string[]>;
  /** Saves the result. Throwing keeps the tour open and shows the error. */
  onFinish: (next: ClientSettings) => Promise<void>;
  /** Closes the tour without changing any settings. */
  onSkip: () => void | Promise<void>;
  allowStartOnLogin?: boolean;
}

export function Onboarding({
  settings,
  loadRecommended,
  pickFolders,
  onFinish,
  onSkip,
  allowStartOnLogin = true,
}: OnboardingProps) {
  const [draft, setDraft] = useState<OnboardingDraft>(() => initialDraft(settings, []));
  const [step, setStep] = useState<StepId>('welcome');
  const [loadingFolders, setLoadingFolders] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Texts follow the language picked in the tour right away, before anything is saved.
  const t = (key: OnboardingKey, values?: Record<string, string | number>) =>
    onboardingText(draft.language, key, values);
  const heading = useRef<HTMLHeadingElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const pending = useRef(false);
  useEffect(() => {
    const element = dialog.current!;
    element.showModal();
    return () => element.close();
  }, []);

  // Suggested folders are fetched once in the background while the user reads step 1.
  useEffect(() => {
    let alive = true;
    loadRecommended()
      .then((folders) => {
        if (!alive) return;
        const suggested = initialDraft(settings, folders).folders;
        setDraft((current) => {
          const known = new Set(current.folders.map((folder) => folderKey(folder.path)));
          return {
            ...current,
            folders: [
              ...suggested.filter((folder) => !known.has(folderKey(folder.path))),
              ...current.folders,
            ],
          };
        });
      })
      .catch(() => undefined)
      .finally(() => alive && setLoadingFolders(false));
    return () => {
      alive = false;
    };
    // Only on first show: later setting changes must not reset the user's choices.
  }, []);

  // Move keyboard focus to the new step's title so screen readers announce it.
  useEffect(() => heading.current?.focus(), [step]);

  const index = steps.indexOf(step);
  const last = index === steps.length - 1;
  const change = (patch: Partial<OnboardingDraft>) =>
    setDraft((current) => ({ ...current, ...patch }));

  const next = async () => {
    if (pending.current) return;
    if (!last) {
      setStep(steps[index + 1]);
      return;
    }
    pending.current = true;
    setSaving(true);
    setError(null);
    try {
      await onFinish(applyDraft(settings, draft));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };
  const skip = async () => {
    if (pending.current) return;
    pending.current = true;
    setSaving(true);
    setError(null);
    try {
      await onSkip();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  return (
    <dialog
      ref={dialog}
      className="onboarding"
      role="dialog"
      aria-modal="true"
      aria-labelledby="onboarding-title"
      lang={draft.language}
      onCancel={(event) => {
        event.preventDefault();
        void skip();
      }}
    >
      <div className="onboarding-card">
        <nav
          className="onboarding-rail"
          aria-label={t('stepOf', { n: index + 1, total: steps.length })}
        >
          <span className="onboarding-brand">
            <Brand wordmark />
          </span>
          <ol>
            {steps.map((id, position) => (
              <li
                key={id}
                className={position < index ? 'done' : position === index ? 'current' : undefined}
                aria-current={position === index ? 'step' : undefined}
              >
                <span className="onboarding-dot" aria-hidden="true">
                  {position < index ? <Check size={12} strokeWidth={2.5} /> : position + 1}
                </span>
                {t(id)}
              </li>
            ))}
          </ol>
        </nav>

        <section className="onboarding-main">
          <div className="onboarding-content">
            {step === 'welcome' ? (
              <WelcomeStep draft={draft} change={change} t={t} heading={heading} />
            ) : step === 'shortcut' ? (
              <ShortcutStep draft={draft} change={change} t={t} heading={heading} />
            ) : step === 'folders' ? (
              <FoldersStep
                draft={draft}
                setDraft={setDraft}
                loading={loadingFolders}
                pickFolders={pickFolders}
                t={t}
                heading={heading}
              />
            ) : step === 'privacy' ? (
              <PrivacyStep
                draft={draft}
                change={change}
                setDraft={setDraft}
                t={t}
                heading={heading}
                allowStartOnLogin={allowStartOnLogin}
              />
            ) : (
              <ReadyStep draft={draft} t={t} heading={heading} />
            )}
          </div>
          {error ? (
            <p className="onboarding-error" role="alert">
              {error}
            </p>
          ) : null}
          <footer className="onboarding-footer">
            {index > 0 ? (
              <button
                className="button"
                onClick={() => setStep(steps[index - 1])}
                disabled={saving}
              >
                {t('back')}
              </button>
            ) : (
              <span />
            )}
            <button className="text-button" onClick={() => void skip()} disabled={saving}>
              {t('skip')}
            </button>
            <button className="button primary" onClick={() => void next()} disabled={saving}>
              {saving ? t('saving') : last ? t('finish') : t('next')}
            </button>
          </footer>
        </section>
      </div>
    </dialog>
  );
}

// ---------- Steps ----------

type Translate = (key: OnboardingKey, values?: Record<string, string | number>) => string;
interface StepProps {
  draft: OnboardingDraft;
  t: Translate;
  heading: React.RefObject<HTMLHeadingElement | null>;
}
type Change = (patch: Partial<OnboardingDraft>) => void;

function Title({ heading, children }: { heading: StepProps['heading']; children: string }) {
  return (
    <h2 id="onboarding-title" ref={heading} tabIndex={-1}>
      {children}
    </h2>
  );
}

function WelcomeStep({ draft, change, t, heading }: StepProps & { change: Change }) {
  const themes: [Theme, OnboardingKey][] = [
    ['system', 'themeSystem'],
    ['light', 'themeLight'],
    ['dark', 'themeDark'],
  ];
  return (
    <>
      <Title heading={heading}>{t('welcomeTitle')}</Title>
      <p className="onboarding-lead">{t('welcomeBody')}</p>
      <fieldset className="onboarding-choice">
        <legend>{t('language')}</legend>
        <div className="onboarding-segments">
          {(Object.keys(languageNames) as Language[]).map((language) => (
            <label key={language} className={draft.language === language ? 'selected' : undefined}>
              <input
                type="radio"
                name="language"
                checked={draft.language === language}
                onChange={() => change({ language })}
              />
              {languageNames[language]}
            </label>
          ))}
        </div>
      </fieldset>
      <fieldset className="onboarding-choice">
        <legend>{t('theme')}</legend>
        <div className="onboarding-segments">
          {themes.map(([theme, label]) => (
            <label key={theme} className={draft.theme === theme ? 'selected' : undefined}>
              <input
                type="radio"
                name="theme"
                checked={draft.theme === theme}
                onChange={() => change({ theme })}
              />
              {t(label)}
            </label>
          ))}
        </div>
      </fieldset>
    </>
  );
}

function Keycaps({ shortcut }: { shortcut: string }) {
  return (
    <span className="onboarding-keys">
      {shortcutKeys(shortcut).map((key, position) => (
        <kbd key={`${key}-${position}`}>{key}</kbd>
      ))}
    </span>
  );
}

function ShortcutStep({ draft, change, t, heading }: StepProps & { change: Change }) {
  const [recording, setRecording] = useState(false);
  const [problem, setProblem] = useState<OnboardingKey | null>(null);
  const box = useRef<HTMLButtonElement>(null);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (!recording) return;
    // While recording, keys belong to the recorder (not to buttons or the browser).
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape') {
      setRecording(false);
      setProblem(null);
      return;
    }
    const result = shortcutFromKey(event);
    if ('shortcut' in result) {
      change({ shortcut: result.shortcut });
      setRecording(false);
      setProblem(null);
    } else if ('error' in result) {
      setProblem(result.error);
    }
  };

  return (
    <>
      <Title heading={heading}>{t('shortcutTitle')}</Title>
      <p className="onboarding-lead">{t('shortcutBody')}</p>
      <button
        ref={box}
        type="button"
        className={`onboarding-recorder${recording ? ' recording' : ''}`}
        onClick={() => {
          setRecording(true);
          setProblem(null);
        }}
        onKeyDown={onKeyDown}
        onBlur={() => setRecording(false)}
        aria-live="polite"
      >
        {recording ? (
          <span className="onboarding-recording">{t('recording')}</span>
        ) : (
          <Keycaps shortcut={draft.shortcut} />
        )}
      </button>
      {problem ? (
        <p className="onboarding-hint error" role="alert">
          {t(problem)}
        </p>
      ) : (
        <p className="onboarding-hint">
          <button
            type="button"
            className="text-button"
            onClick={() => {
              setRecording(true);
              box.current?.focus();
            }}
          >
            {t('change')}
          </button>
        </p>
      )}
    </>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label className={`onboarding-toggle${disabled ? ' disabled' : ''}`}>
      <input
        type="checkbox"
        role="switch"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="onboarding-switch" aria-hidden="true" />
      <span>
        <strong>{label}</strong>
        {hint ? <small>{hint}</small> : null}
      </span>
    </label>
  );
}

function FoldersStep({
  draft,
  setDraft,
  loading,
  pickFolders,
  t,
  heading,
}: StepProps & {
  setDraft: React.Dispatch<React.SetStateAction<OnboardingDraft>>;
  loading: boolean;
  pickFolders: () => Promise<string[]>;
}) {
  const updateFolder = (path: string, patch: Partial<FolderChoice>) =>
    setDraft((current) => ({
      ...current,
      folders: current.folders.map((folder) =>
        folder.path === path ? { ...folder, ...patch } : folder,
      ),
    }));
  const add = async () => {
    const paths = await pickFolders().catch(() => [] as string[]);
    setDraft((current) => paths.reduce(addFolder, current));
  };
  return (
    <>
      <Title heading={heading}>{t('foldersTitle')}</Title>
      <p className="onboarding-lead">{t('foldersBody')}</p>
      <ul className="onboarding-folders">
        {draft.folders.map((folder) => (
          <li key={folder.path} className={folder.selected ? undefined : 'off'}>
            <label className="onboarding-folder-name">
              <input
                type="checkbox"
                checked={folder.selected}
                onChange={(event) => updateFolder(folder.path, { selected: event.target.checked })}
              />
              <span>
                <strong>{folder.name}</strong>
                <small title={folder.path}>{folder.path}</small>
              </span>
            </label>
            <label className="onboarding-inline">
              <input
                type="checkbox"
                checked={folder.content}
                disabled={!folder.selected}
                onChange={(event) => updateFolder(folder.path, { content: event.target.checked })}
              />
              {t('documentText')}
            </label>
          </li>
        ))}
      </ul>
      {loading ? <p className="onboarding-hint">{t('loadingFolders')}</p> : null}
      {!loading && draft.folders.length === 0 ? (
        <p className="onboarding-hint">{t('noFolders')}</p>
      ) : null}
      <button type="button" className="button compact" onClick={() => void add()}>
        <FolderPlus size={15} />
        {t('addFolder')}
      </button>
    </>
  );
}

function PrivacyStep({
  draft,
  change,
  setDraft,
  t,
  heading,
  allowStartOnLogin,
}: StepProps & {
  change: Change;
  setDraft: React.Dispatch<React.SetStateAction<OnboardingDraft>>;
  allowStartOnLogin: boolean;
}) {
  // Only folders whose document text is read can contribute text to an AI answer.
  const contentFolders = draft.folders.filter((folder) => folder.selected && folder.content);
  return (
    <>
      <Title heading={heading}>{t('privacyTitle')}</Title>
      <div className="onboarding-toggles">
        <Toggle
          checked={draft.clipboardEnabled}
          onChange={(clipboardEnabled) => change({ clipboardEnabled })}
          label={t('clipboardHistory')}
          hint={t('clipboardHistoryHint')}
        />
        <div className="onboarding-nested">
          <Toggle
            checked={draft.clipboardEnabled && draft.clipboardImages}
            disabled={!draft.clipboardEnabled}
            onChange={(clipboardImages) => change({ clipboardImages })}
            label={t('clipboardImages')}
          />
        </div>
        <Toggle
          checked={draft.startOnLogin}
          disabled={!allowStartOnLogin}
          onChange={(startOnLogin) => change({ startOnLogin })}
          label={t('startOnLogin')}
          hint={t(allowStartOnLogin ? 'startOnLoginHint' : 'portableStartup')}
        />
        <Toggle
          checked={draft.allowExternalQuestions}
          onChange={(allowExternalQuestions) => change({ allowExternalQuestions })}
          label={t('externalQuestions')}
          hint={t('externalQuestionsHint')}
        />
        {draft.allowExternalQuestions && contentFolders.length ? (
          <div className="onboarding-nested">
            {contentFolders.map((folder) => (
              <Toggle
                key={folder.path}
                checked={folder.external}
                onChange={(external) =>
                  setDraft((current) => ({
                    ...current,
                    folders: current.folders.map((item) =>
                      item.path === folder.path ? { ...item, external } : item,
                    ),
                  }))
                }
                label={folder.name}
                hint={t('folderExternal')}
              />
            ))}
          </div>
        ) : null}
      </div>
      <p className="onboarding-note">{t('modelsNote')}</p>
    </>
  );
}

function ReadyStep({ draft, t, heading }: StepProps) {
  const tips = useMemo(
    () => [t('tipFile'), t('tipCalc'), ...(draft.clipboardEnabled ? [t('tipClipboard')] : [])],
    // t depends only on the language
    [draft.language, draft.clipboardEnabled],
  );
  const [before, after] = t('tipShortcut').split('{keys}');
  return (
    <>
      <Title heading={heading}>{t('readyTitle')}</Title>
      <p className="onboarding-lead">{t('readyBody')}</p>
      <ul className="onboarding-tips">
        <li>
          {before}
          <Keycaps shortcut={draft.shortcut} />
          {after}
        </li>
        {tips.map((tip) => (
          <li key={tip}>{tip}</li>
        ))}
      </ul>
    </>
  );
}
