import { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Search,
  MessageSquare,
  FileText,
  Zap,
  Minus,
  Square,
  X,
  Check,
  AlertCircle,
} from 'lucide-react';
import { Brand } from './components/Brand';
import { Sidebar } from './components/Sidebar';
import { SettingsDialog } from './components/SettingsDialog';
import type { SettingsTab } from './components/SettingsDialog';
import { RecommendedDialog } from './components/RecommendedDialog';
import { SearchView } from './features/SearchView';
import { ChatView } from './features/ChatView';
import { DocumentsView } from './features/DocumentsView';
import { Onboarding } from './features/onboarding/Onboarding';
import { QuickWindow } from './features/QuickWindow';
import { ClipboardNotice } from './features/clipboard/ClipboardPanel';
import { bridge, native } from './lib/bridge';
import {
  useClient,
  initialize,
  saveSettings,
  addFolders,
  showToast,
  clearToast,
  errorMessage,
  switchWindow,
} from './lib/store';
import { languageNames, translate } from './lib/i18n';
import { isComposing } from './lib/types';
import type { RecommendedFolder, Tab } from './lib/types';

function useQuickMode() {
  const nativeQuick = native && getCurrentWindow().label === 'quick';
  const [quick, setQuick] = useState(nativeQuick || window.location.hash === '#quick');
  useEffect(() => {
    if (native) return;
    const change = () => setQuick(window.location.hash === '#quick');
    window.addEventListener('hashchange', change);
    return () => window.removeEventListener('hashchange', change);
  }, []);
  useEffect(() => {
    document.body.classList.toggle('native-quick', nativeQuick);
    document.documentElement.classList.toggle('native-quick', nativeQuick);
  }, [nativeQuick]);
  return quick;
}

export default function App() {
  const state = useClient();
  const { settings, runtime } = state;
  const [tab, setTab] = useState<Tab>('search');
  const [documentDirty, setDocumentDirty] = useState(false);
  const [recent, setRecent] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [recommended, setRecommended] = useState<RecommendedFolder[] | null>(null);
  const quick = useQuickMode();
  const needsOnboarding = settings.onboardingVersion < 1;
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const changeTab = (next: Tab) => {
    if (tab === 'documents' && next !== tab && documentDirty) {
      showToast(t('documentUnsaved'));
      return false;
    }
    setTab(next);
    return true;
  };
  useEffect(() => {
    void initialize();
  }, []);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void bridge
      .subscribeSettings((tab) => {
        if (['general', 'scopes', 'shortcuts', 'ai'].includes(tab)) setSettingsTab(tab);
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch((cause) => showToast(errorMessage(cause)));
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      document.documentElement.dataset.theme =
        settings.theme === 'system' ? (media.matches ? 'dark' : 'light') : settings.theme;
    };
    apply();
    media.addEventListener('change', apply);
    document.documentElement.style.fontSize = `${settings.fontSize}px`;
    document.documentElement.lang = settings.language;
    return () => media.removeEventListener('change', apply);
  }, [settings.theme, settings.fontSize, settings.language]);
  useEffect(() => {
    if (!state.toast) return;
    const timer = setTimeout(clearToast, 4200);
    return () => clearTimeout(timer);
  }, [state.toast]);
  useEffect(() => {
    if (runtime.shortcutError)
      showToast(`${translate(settings.language, 'shortcutConflict')} ${runtime.shortcutError}`);
  }, [runtime.shortcutError, settings.language]);
  useEffect(() => {
    if (runtime.startupError)
      showToast(`${translate(settings.language, 'startOnLogin')}: ${runtime.startupError}`);
  }, [runtime.startupError, settings.language]);
  useEffect(() => {
    if (native) return;
    const handler = (event: KeyboardEvent) => {
      if (
        !isComposing(event) &&
        event.altKey &&
        event.code === 'Space' &&
        !settingsTab &&
        !recommended &&
        !needsOnboarding
      ) {
        event.preventDefault();
        void (quick ? switchWindow('main') : switchWindow('quick')).catch((cause) =>
          showToast(errorMessage(cause)),
        );
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [quick, settingsTab, recommended, needsOnboarding]);
  const add = () => {
    void addFolders()
      .then((added) => {
        if (added) showToast(t('folderAdded'));
      })
      .catch((cause) =>
        showToast(errorMessage(cause) === 'NATIVE_ONLY' ? t('nativeOnly') : errorMessage(cause)),
      );
  };
  const chooseRecommended = () => {
    void bridge
      .recommendedFolders()
      .then((folders) => {
        if (folders.length) setRecommended(folders);
        else showToast(runtime.native ? t('recommendedUnavailable') : t('nativeOnly'));
      })
      .catch((cause) => showToast(errorMessage(cause)));
  };
  const windowAction = (action: 'minimize' | 'maximize' | 'close') => {
    if (!native) {
      showToast(t('nativeOnly'));
      return;
    }
    void bridge.windowAction(action).catch((cause) => showToast(errorMessage(cause)));
  };
  if (state.error)
    return (
      <main className="load-state">
        <AlertCircle size={30} />
        <h1>{t('loadError')}</h1>
        <p>{state.error}</p>
        <button className="button" onClick={() => void initialize()}>
          {t('retry')}
        </button>
      </main>
    );
  if (!state.ready)
    return (
      <main className="load-state">
        <Brand wordmark />
        <span className="loading-dot" />
      </main>
    );
  return (
    <>
      {quick ? (
        <QuickWindow />
      ) : (
        <div className="app-shell">
          <header className="titlebar" data-tauri-drag-region>
            <div className="titlebar-brand" data-tauri-drag-region>
              <Brand wordmark />
            </div>
            <nav className="main-tabs" aria-label="Prism">
              {(
                [
                  { id: 'search', icon: Search },
                  { id: 'chat', icon: MessageSquare },
                  { id: 'documents', icon: FileText },
                ] as const
              ).map((item) => (
                <button
                  key={item.id}
                  className={`main-tab ${tab === item.id ? 'active' : ''}`}
                  aria-current={tab === item.id ? 'page' : undefined}
                  onClick={() => changeTab(item.id)}
                >
                  <item.icon size={19} />
                  {t(item.id)}
                </button>
              ))}
            </nav>
            <div className="titlebar-spacer" data-tauri-drag-region />
            <button
              className="button quick-launch"
              aria-label={t('quickSearch')}
              onClick={() =>
                void switchWindow('quick').catch((cause) => showToast(errorMessage(cause)))
              }
            >
              <Zap size={17} />
              <span>{t('quickSearch')}</span>
              <kbd>{settings.shortcut.replaceAll('+', ' ')}</kbd>
            </button>
            <div className="window-controls">
              <button aria-label="Minimize" onClick={() => windowAction('minimize')}>
                <Minus size={17} />
              </button>
              <button aria-label="Maximize" onClick={() => windowAction('maximize')}>
                <Square size={14} />
              </button>
              <button
                className="close-window"
                aria-label="Close window"
                onClick={() => windowAction('close')}
              >
                <X size={18} />
              </button>
            </div>
          </header>
          <div className="workspace">
            <Sidebar
              recent={recent}
              onRecent={(value) => {
                if (changeTab('search')) setRecent(value);
              }}
              onSettings={() => setSettingsTab('general')}
              onAddFolder={add}
            />
            <main className="main-content">
              {!needsOnboarding ? <ClipboardNotice /> : null}
              {tab === 'search' ? (
                <SearchView
                  recent={recent}
                  onRecent={setRecent}
                  onAddFolder={add}
                  onRecommended={chooseRecommended}
                  onSettings={() => setSettingsTab('scopes')}
                />
              ) : tab === 'chat' ? (
                <ChatView onSettings={() => setSettingsTab('scopes')} />
              ) : (
                <DocumentsView onDirtyChange={setDocumentDirty} />
              )}
            </main>
          </div>
          <footer className="statusbar">
            <span>
              <i className="status-dot" />
              {runtime.native ? t('localWorkspace') : t('browserPreview')}
            </span>
            <div>
              <button onClick={() => setSettingsTab('general')}>
                {languageNames[settings.language]}
              </button>
              <span className="status-separator" />
              <button onClick={() => setSettingsTab('general')}>
                {settings.theme === 'system' ? t('systemTheme') : t(settings.theme)}
              </button>
            </div>
          </footer>
        </div>
      )}
      {!quick && needsOnboarding ? (
        <Onboarding
          settings={settings}
          loadRecommended={bridge.recommendedFolders}
          pickFolders={bridge.chooseFolders}
          onFinish={(next) => saveSettings({ ...next, onboardingVersion: 1 })}
          onSkip={() => saveSettings({ ...settings, onboardingVersion: 1 })}
        />
      ) : null}
      {settingsTab ? (
        <SettingsDialog initialTab={settingsTab} onClose={() => setSettingsTab(null)} />
      ) : null}
      {recommended ? (
        <RecommendedDialog folders={recommended} onClose={() => setRecommended(null)} />
      ) : null}
      {state.toast ? (
        <div className="toast" role="status">
          <Check size={16} />
          <span>{state.toast}</span>
          <button className="icon-button small" aria-label={t('close')} onClick={clearToast}>
            <X size={15} />
          </button>
        </div>
      ) : null}
    </>
  );
}
