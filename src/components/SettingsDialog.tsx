import { useEffect, useRef, useState } from 'react';
import { X, Plus, Trash2, Folder, Settings2, Keyboard, Folders, LogOut, Cpu } from 'lucide-react';
import { EmbeddingPanel } from '../features/models/EmbeddingPanel';
import { GenerationPanel } from '../features/generation/GenerationPanel';
import { UpdatePanel } from '../features/updates/UpdatePanel';
import { DiagnosticPanel } from '../features/diagnostics/DiagnosticPanel';
import { OcrSettings } from '../features/content/OcrSettings';
import { IndexServicePanel } from '../features/index-service/IndexServicePanel';
import { generationCredentials } from '../lib/rag';
import { bridge } from '../lib/bridge';
import { useClient, saveSettings, scopesFromPaths, showToast, errorMessage } from '../lib/store';
import { languageNames, translate } from '../lib/i18n';
import { fileShortcutError } from '../lib/file-shortcut';
import type { Language, Theme, DocumentMode } from '../lib/types';
import './settings-dialog.css';
import { McpSettings } from './mcp/McpSettings';
export type SettingsTab = 'general' | 'scopes' | 'shortcuts' | 'ai';

export function SettingsDialog({
  initialTab,
  onClose,
  allowUpdateInstall = true,
}: {
  initialTab: SettingsTab;
  onClose: () => void;
  allowUpdateInstall?: boolean;
}) {
  const { settings, runtime } = useClient();
  const [draft, setDraft] = useState(() => structuredClone(settings));
  const [tab, setTab] = useState(initialTab);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const shortcutError = fileShortcutError(draft.fileOpenShortcut, draft.shortcut, runtime.platform);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  const submit = async () => {
    if (shortcutError) {
      setTab('shortcuts');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await saveSettings(draft);
      showToast(translate(draft.language, 'saved'));
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };
  const addScope = async () => {
    try {
      const paths = await bridge.chooseFolders();
      setDraft((value) => ({ ...value, scopes: scopesFromPaths(paths, value.scopes) }));
    } catch (cause) {
      setError(errorMessage(cause) === 'NATIVE_ONLY' ? t('nativeOnly') : errorMessage(cause));
    }
  };
  return (
    <dialog
      ref={dialog}
      className="modal settings-modal"
      aria-labelledby="settings-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget && !busy) onClose();
      }}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <div className="modal-header">
          <div>
            <Settings2 size={20} />
            <h2 id="settings-title">{t('settings')}</h2>
          </div>
          <button type="button" className="icon-button" aria-label={t('close')} onClick={onClose}>
            <X size={19} />
          </button>
        </div>
        <div className="settings-layout">
          <nav className="settings-nav" aria-label={t('settings')}>
            {(
              [
                { id: 'general', icon: Settings2 },
                { id: 'scopes', icon: Folders },
                { id: 'shortcuts', icon: Keyboard },
                { id: 'ai', icon: Cpu },
              ] as const
            ).map((item) => (
              <button
                type="button"
                key={item.id}
                className={`rail-item ${tab === item.id ? 'active' : ''}`}
                onClick={() => setTab(item.id)}
              >
                <item.icon size={17} />
                {t(item.id)}
              </button>
            ))}
          </nav>
          <div className="settings-body">
            {tab === 'ai' ? (
              <>
                <EmbeddingPanel />
                <label className="setting-check">
                  <span>{t('automaticSemantic')}</span>
                  <input
                    type="checkbox"
                    checked={draft.automaticSemantic}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        automaticSemantic: event.target.checked,
                      }))
                    }
                  />
                </label>
                <p className="setting-footnote">{t('automaticSemanticHint')}</p>
                <label className="setting-check">
                  <span>{t('hybridSearch')}</span>
                  <input
                    type="checkbox"
                    checked={draft.hybridSearch}
                    onChange={(event) =>
                      setDraft((current) => ({ ...current, hybridSearch: event.target.checked }))
                    }
                  />
                </label>
                <p className="setting-footnote">{t('hybridSearchHint')}</p>
                <GenerationPanel
                  preferences={draft.generation}
                  onChange={(generation) => setDraft((current) => ({ ...current, generation }))}
                  language={settings.language}
                  native={runtime.native}
                  credentials={generationCredentials}
                />
                <McpSettings
                  value={draft.mcp}
                  saved={settings.mcp}
                  onChange={(mcp) => setDraft((current) => ({ ...current, mcp }))}
                  language={settings.language}
                  native={runtime.native}
                />
              </>
            ) : null}
            {tab === 'general' ? (
              <>
                <div className="setting-field">
                  <label htmlFor="theme">{t('theme')}</label>
                  <select
                    id="theme"
                    value={draft.theme}
                    onChange={(event) => setDraft({ ...draft, theme: event.target.value as Theme })}
                  >
                    <option value="system">{t('system')}</option>
                    <option value="light">{t('light')}</option>
                    <option value="dark">{t('dark')}</option>
                  </select>
                </div>
                <div className="setting-field">
                  <label htmlFor="language">{t('language')}</label>
                  <select
                    id="language"
                    value={draft.language}
                    onChange={(event) =>
                      setDraft({ ...draft, language: event.target.value as Language })
                    }
                  >
                    {Object.entries(languageNames).map(([code, name]) => (
                      <option key={code} value={code}>
                        {name}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="setting-field">
                  <label htmlFor="font-size">{t('fontSize')}</label>
                  <div className="range-field">
                    <input
                      id="font-size"
                      type="range"
                      min="12"
                      max="18"
                      step="1"
                      value={draft.fontSize}
                      onChange={(event) =>
                        setDraft({ ...draft, fontSize: Number(event.target.value) })
                      }
                    />
                    <output htmlFor="font-size">{draft.fontSize}px</output>
                  </div>
                </div>
                <div className="setting-field">
                  <label htmlFor="document-mode">{t('documentMode')}</label>
                  <select
                    id="document-mode"
                    value={draft.documents.defaultMode}
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        documents: { defaultMode: event.target.value as DocumentMode },
                      })
                    }
                  >
                    <option value="new">{t('documentModeNew')}</option>
                    <option value="fill">{t('documentModeFill')}</option>
                  </select>
                </div>
                <div className="settings-divider" />
                <div className="setting-field">
                  <label htmlFor="calc-precision">{t('precision')}</label>
                  <input
                    id="calc-precision"
                    type="number"
                    min="6"
                    max="15"
                    value={draft.calculator.precision}
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        calculator: {
                          ...draft.calculator,
                          precision: Math.min(15, Math.max(6, Number(event.target.value) || 12)),
                        },
                      })
                    }
                  />
                </div>
                <div className="setting-field">
                  <label htmlFor="calc-angle">{t('angle')}</label>
                  <select
                    id="calc-angle"
                    value={draft.calculator.angle}
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        calculator: {
                          ...draft.calculator,
                          angle: event.target.value as 'rad' | 'deg',
                        },
                      })
                    }
                  >
                    <option value="rad">{t('radians')}</option>
                    <option value="deg">{t('degrees')}</option>
                  </select>
                </div>
                <div className="settings-divider" />
                <label className="setting-check">
                  <span>{t('rememberSearch')}</span>
                  <input
                    type="checkbox"
                    checked={draft.rememberSearch}
                    onChange={(event) =>
                      setDraft({ ...draft, rememberSearch: event.target.checked })
                    }
                  />
                </label>
                <label className="setting-check">
                  <span>{t('closeToTray')}</span>
                  <input
                    type="checkbox"
                    checked={draft.closeToTray}
                    onChange={(event) => setDraft({ ...draft, closeToTray: event.target.checked })}
                  />
                </label>
                <label className="setting-check">
                  <span>{t('keyboardCorrection')}</span>
                  <input
                    type="checkbox"
                    checked={draft.keyboardCorrection}
                    onChange={(event) =>
                      setDraft({ ...draft, keyboardCorrection: event.target.checked })
                    }
                  />
                </label>
                <p className="setting-footnote">{t('keyboardCorrectionHint')}</p>
                <label className="setting-check">
                  <span>{t('startOnLogin')}</span>
                  <input
                    type="checkbox"
                    checked={draft.startOnLogin}
                    disabled={runtime.installKind === 'portable'}
                    onChange={(event) => setDraft({ ...draft, startOnLogin: event.target.checked })}
                  />
                </label>
                {runtime.installKind === 'portable' ? (
                  <p className="setting-footnote">{t('portableStartup')}</p>
                ) : runtime.installKind === 'packaged' && !runtime.development ? (
                  <p className="setting-footnote">{t('packagedStartup')}</p>
                ) : runtime.development ? (
                  <p className="setting-footnote">{t('developmentStartup')}</p>
                ) : null}
                <UpdatePanel
                  autoCheck={draft.updates.autoCheck}
                  onAutoCheck={(autoCheck) => setDraft({ ...draft, updates: { autoCheck } })}
                  installBlocked={
                    !allowUpdateInstall || JSON.stringify(draft) !== JSON.stringify(settings)
                  }
                />
                <DiagnosticPanel language={settings.language} />
              </>
            ) : null}
            {tab === 'scopes' ? (
              <>
                <div className="section-title">
                  <h3>{t('searchFolders')}</h3>
                  <button type="button" className="button compact" onClick={() => void addScope()}>
                    <Plus size={15} />
                    {t('addFolder')}
                  </button>
                </div>
                <p className="setting-description">{t('scopeDescription')}</p>
                {draft.scopes.length ? (
                  <div className="scope-list">
                    {draft.scopes.map((scope) => (
                      <div className="scope-setting" key={scope.id}>
                        <div className="scope-setting-title">
                          <Folder size={18} />
                          <div>
                            <strong>{scope.name}</strong>
                            <span title={scope.path}>{scope.path}</span>
                          </div>
                          <button
                            type="button"
                            className="icon-button"
                            aria-label={`${t('remove')}: ${scope.name}`}
                            onClick={() =>
                              setDraft({
                                ...draft,
                                scopes: draft.scopes.filter((item) => item.id !== scope.id),
                              })
                            }
                          >
                            <Trash2 size={16} />
                          </button>
                        </div>
                        <div className="scope-checkboxes">
                          <label>
                            <input
                              type="checkbox"
                              checked={scope.filenameEnabled}
                              onChange={(event) =>
                                setDraft({
                                  ...draft,
                                  scopes: draft.scopes.map((item) =>
                                    item.id === scope.id
                                      ? { ...item, filenameEnabled: event.target.checked }
                                      : item,
                                  ),
                                })
                              }
                            />
                            {t('filenameScope')}
                          </label>
                          <label>
                            <input
                              type="checkbox"
                              checked={scope.contentEnabled}
                              onChange={(event) =>
                                setDraft({
                                  ...draft,
                                  scopes: draft.scopes.map((item) =>
                                    item.id === scope.id
                                      ? {
                                          ...item,
                                          contentEnabled: event.target.checked,
                                          externalAllowed:
                                            event.target.checked && item.externalAllowed,
                                        }
                                      : item,
                                  ),
                                })
                              }
                            />
                            {t('contentScope')}
                          </label>
                          <label>
                            <input
                              type="checkbox"
                              checked={scope.externalAllowed}
                              disabled={!scope.contentEnabled}
                              onChange={(event) =>
                                setDraft({
                                  ...draft,
                                  scopes: draft.scopes.map((item) =>
                                    item.id === scope.id
                                      ? { ...item, externalAllowed: event.target.checked }
                                      : item,
                                  ),
                                })
                              }
                            />
                            {t('externalScope')}
                          </label>
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="scope-empty">{t('noScopes')}</p>
                )}
                <p className="setting-footnote">{t('engineNext')}</p>
                <IndexServicePanel language={settings.language} runtime={runtime} />
                {/* Off by default; filename search and searching itself are never paused. */}
                <label className="setting-check">
                  <span>
                    {t('pauseOnBattery')}
                    <small className="setting-hint">{t('pauseOnBatteryHint')}</small>
                  </span>
                  <input
                    type="checkbox"
                    checked={draft.power.pauseOnBattery}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        power: { pauseOnBattery: event.target.checked },
                      }))
                    }
                  />
                </label>
                <div className="settings-divider" />
                <OcrSettings
                  value={draft.ocr}
                  onChange={(ocr) => setDraft((current) => ({ ...current, ocr }))}
                  language={settings.language}
                  native={runtime.native}
                />
              </>
            ) : null}
            {tab === 'shortcuts' ? (
              <>
                <h3>{t('quickSearch')}</h3>
                <p className="setting-description">{t('shortcutDescription')}</p>
                <label className="shortcut-label" htmlFor="shortcut">
                  {t('shortcuts')}
                </label>
                <input
                  id="shortcut"
                  className="shortcut-input"
                  value={draft.shortcut}
                  onChange={(event) => setDraft({ ...draft, shortcut: event.target.value })}
                />
                <p className="setting-footnote">{t('shortcutHint')}</p>
                <label className="shortcut-label" htmlFor="file-open-shortcut">
                  {t('fileOpenShortcut')}
                </label>
                <input
                  id="file-open-shortcut"
                  className="shortcut-input"
                  value={draft.fileOpenShortcut}
                  maxLength={64}
                  autoComplete="off"
                  spellCheck={false}
                  aria-invalid={!!shortcutError}
                  aria-describedby="file-shortcut-hint file-shortcut-error"
                  onChange={(event) => setDraft({ ...draft, fileOpenShortcut: event.target.value })}
                />
                <p id="file-shortcut-hint" className="setting-footnote">
                  {t('fileShortcutHint')}
                </p>
                <p id="file-shortcut-error" className="form-error" role="status">
                  {shortcutError ? t(shortcutError) : null}
                </p>
                {!runtime.native ? <p className="setting-footnote">{t('nativeOnly')}</p> : null}
              </>
            ) : null}
            {error ? (
              <p className="form-error" role="alert">
                {error}
              </p>
            ) : null}
          </div>
        </div>
        <div className="modal-footer">
          <span>{t('settingsHint')}</span>
          <button type="button" className="button" onClick={onClose} disabled={busy}>
            {t('cancel')}
          </button>
          <button className="button primary" disabled={busy || !!shortcutError}>
            {t('save')}
          </button>
        </div>
        {runtime.native ? (
          <button
            type="button"
            className="quit-button"
            onClick={() => void bridge.quit().catch((cause) => setError(errorMessage(cause)))}
          >
            <LogOut size={14} />
            {t('quit')}
          </button>
        ) : null}
      </form>
    </dialog>
  );
}
