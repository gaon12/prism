import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
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
  PanelLeft,
  Files,
  History,
  Settings2,
  BrainCircuit,
  ListChecks,
  LibraryBig,
} from 'lucide-react';
import { Brand } from './components/Brand';
import { Sidebar } from './components/Sidebar';
import type { SettingsTab } from './components/SettingsDialog';
import { RecommendedDialog } from './components/RecommendedDialog';
import { SearchView } from './features/SearchView';
import { QuickWindow } from './features/QuickWindow';
import { ClipboardNotice } from './features/clipboard/ClipboardPanel';
import { UpdateBanner } from './features/updates/UpdatePanel';
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
import { languageNames, startupFailure, translate } from './lib/i18n';
import { isComposing } from './lib/types';
import type { RecommendedFolder, Tab } from './lib/types';
import { rag } from './lib/rag';
import { content } from './lib/content';
import { IndexActivityProvider } from './lib/index-activity';
import { IndexingStatus, IndexingTasks } from './components/IndexingTasks';
import { workspaceText } from './components/workspace-messages';
import type { ChatSelection } from './lib/rag';

// Screens only the main window shows, loaded on first use. The quick search
// window renders from the same bundle, and on small (512 MB) machines every
// WebView renderer pays to parse and keep the code it loads. Search, the
// default tab, and the quick window stay in the main bundle so they open at once.
const ChatView = lazy(() => import('./features/ChatView').then((m) => ({ default: m.ChatView })));
const DocumentsView = lazy(() =>
  import('./features/DocumentsView').then((m) => ({ default: m.DocumentsView })),
);
const CollectionsView = lazy(() =>
  import('./features/collections/CollectionsView').then((m) => ({ default: m.CollectionsView })),
);
const Onboarding = lazy(() =>
  import('./features/onboarding/Onboarding').then((m) => ({ default: m.Onboarding })),
);
const SettingsDialog = lazy(() =>
  import('./components/SettingsDialog').then((m) => ({ default: m.SettingsDialog })),
);

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
function useCompactWorkspace() {
  const [compact, setCompact] = useState(() => window.matchMedia('(max-width: 800px)').matches);
  useEffect(() => {
    const media = window.matchMedia('(max-width: 800px)');
    const update = () => setCompact(media.matches);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  return compact;
}

export default function App() {
  const state = useClient();
  const { settings, runtime } = state;
  const [tab, setTab] = useState<Tab>('search');
  const searchContext = useRef<ChatSelection | null>(null);
  const [initialChat, setInitialChat] = useState<string | null>(null);
  const [incomingChat, setIncomingChat] = useState<string | null>(null);
  const chatPending = useRef(false);
  const creationRequest = useRef<string | null>(null);
  const [creatingSearchChat, setCreatingSearchChat] = useState(false);
  const tabIntent = useRef(0);
  const contextChanged = useCallback((selection: ChatSelection | null) => {
    searchContext.current = selection;
  }, []);
  const [documentDirty, setDocumentDirty] = useState(false);
  const [recent, setRecent] = useState(false);
  const [libraryOpen, setLibraryOpen] = useState(true);
  const [mobileLibraryOpen, setMobileLibraryOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const libraryButton = useRef<HTMLButtonElement>(null);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [recommended, setRecommended] = useState<RecommendedFolder[] | null>(null);
  const quick = useQuickMode();
  const compactWorkspace = useCompactWorkspace();
  const needsOnboarding = settings.onboardingVersion < 1;
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const w = (key: Parameters<typeof workspaceText>[1]) => workspaceText(settings.language, key);
  const toggleLibrary = () => {
    if (compactWorkspace) setMobileLibraryOpen((v) => !v);
    else setLibraryOpen((v) => !v);
  };
  const closeMobileLibrary = () => {
    setMobileLibraryOpen(false);
    libraryButton.current?.focus();
  };
  useEffect(() => {
    if (!mobileLibraryOpen) return;
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        setMobileLibraryOpen(false);
        libraryButton.current?.focus();
      }
    };
    window.addEventListener('keydown', dismiss);
    return () => window.removeEventListener('keydown', dismiss);
  }, [mobileLibraryOpen]);
  const changeTab = (next: Tab) => {
    if (next === 'chat' && chatPending.current) return false;
    tabIntent.current++;
    if (next !== 'chat' && creationRequest.current)
      void content.cancel(creationRequest.current).catch(() => {});
    if (tab === 'documents' && next !== tab && documentDirty) {
      showToast(t('documentUnsaved'));
      return false;
    }
    if (next === 'chat' && tab === 'search' && native && searchContext.current) {
      void openSearchChat();
    } else setTab(next);
    return true;
  };
  const openSearchChat = async (provided?: ChatSelection) => {
    const selection = provided ?? searchContext.current;
    if (!native || !selection || chatPending.current) return;
    chatPending.current = true;
    setCreatingSearchChat(true);
    const intent = ++tabIntent.current;
    const requestId = crypto.randomUUID();
    creationRequest.current = requestId;
    try {
      const conversation = await rag.createSearch(selection, requestId);
      if (intent === tabIntent.current) {
        if (quick) await rag.openMainChat(conversation.id);
        else {
          setInitialChat(conversation.id);
          setTab('chat');
        }
      } else await rag.remove(conversation.id);
    } catch (cause) {
      if (intent === tabIntent.current) showToast(errorMessage(cause));
    } finally {
      chatPending.current = false;
      creationRequest.current = null;
      setCreatingSearchChat(false);
    }
  };
  useEffect(() => {
    void initialize();
  }, []);
  useEffect(() => {
    if (!native || quick) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    const take = async () => {
      try {
        const id = await rag.takeChatHandoff();
        if (!disposed && id) setIncomingChat(id);
      } catch (cause) {
        if (!disposed) showToast(errorMessage(cause));
      }
    };
    void rag
      .subscribeChatHandoff(() => void take())
      .then((unlisten) => {
        if (disposed) unlisten();
        else {
          stop = unlisten;
          void take();
        }
      })
      .catch((cause) => {
        if (!disposed) showToast(errorMessage(cause));
      });
    return () => {
      disposed = true;
      stop?.();
    };
  }, [quick]);
  useEffect(() => {
    if (!incomingChat || !state.ready || quick || needsOnboarding || documentDirty) return;
    tabIntent.current++;
    if (creationRequest.current) void content.cancel(creationRequest.current).catch(() => {});
    setInitialChat(incomingChat);
    setTab('chat');
    setIncomingChat(null);
  }, [incomingChat, state.ready, quick, needsOnboarding, documentDirty]);
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
      showToast(
        `${translate(settings.language, 'startOnLogin')}: ${startupFailure(settings.language, runtime.startupError)}`,
      );
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
    <IndexActivityProvider active={!quick}>
      {quick ? (
        <QuickWindow
          onContextChange={contextChanged}
          onChat={(selection) => void openSearchChat(selection)}
          creatingChat={creatingSearchChat}
          onCancelChat={() => {
            tabIntent.current++;
            if (creationRequest.current)
              void content.cancel(creationRequest.current).catch(() => {});
          }}
        />
      ) : (
        <div className="app-shell ide-shell">
          <header className="titlebar" data-tauri-drag-region>
            <button
              ref={libraryButton}
              type="button"
              className="icon-button workspace-library-toggle"
              title={w('libraryToggle')}
              aria-label={w('libraryToggle')}
              aria-controls="workspace-library"
              aria-expanded={compactWorkspace ? mobileLibraryOpen : libraryOpen}
              onClick={toggleLibrary}
            >
              <PanelLeft size={18} />
            </button>
            <div className="titlebar-brand" data-tauri-drag-region>
              <Brand wordmark />
            </div>
            <span className="workspace-breadcrumb" data-tauri-drag-region>
              {settings.scopes.find((scope) => scope.id === state.session.scopeId)?.name ??
                t('allFiles')}
            </span>
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
            <nav className="workspace-tool-bar" aria-label={w('tools')}>
              <button
                type="button"
                className={`workspace-tool ${(compactWorkspace ? mobileLibraryOpen : libraryOpen) ? 'active' : ''}`}
                title={t('library')}
                aria-label={t('library')}
                aria-controls="workspace-library"
                onClick={toggleLibrary}
              >
                <Files size={20} />
              </button>
              <button
                type="button"
                className={`workspace-tool ${recent && tab === 'search' ? 'active' : ''}`}
                title={w('recentTool')}
                aria-label={w('recentTool')}
                onClick={() => {
                  if (changeTab('search')) setRecent(true);
                }}
              >
                <History size={20} />
              </button>
              <span className="workspace-tool-spacer" />
              <button
                type="button"
                className="workspace-tool"
                title={t('settings')}
                aria-label={t('settings')}
                onClick={() => setSettingsTab('general')}
              >
                <Settings2 size={20} />
              </button>
            </nav>
            {mobileLibraryOpen ? (
              <button
                type="button"
                className="workspace-panel-backdrop"
                aria-label={t('close')}
                onClick={closeMobileLibrary}
              />
            ) : null}
            <div
              id="workspace-library"
              className="workspace-library"
              data-open={libraryOpen}
              data-mobile-open={mobileLibraryOpen}
            >
              <div className="workspace-panel-heading">
                <strong>{t('library')}</strong>
                <button
                  type="button"
                  className="icon-button small"
                  aria-label={w('libraryToggle')}
                  onClick={() => {
                    if (mobileLibraryOpen) closeMobileLibrary();
                    else setLibraryOpen(false);
                  }}
                >
                  <PanelLeft size={15} />
                </button>
              </div>
              <Sidebar
                recent={recent}
                onRecent={(value) => {
                  if (changeTab('search')) {
                    setRecent(value);
                    setMobileLibraryOpen(false);
                  }
                }}
                onSettings={() => setSettingsTab('general')}
                onAddFolder={add}
              />
            </div>
            <main className="main-content">
              <nav className="main-tabs workspace-tabs" aria-label="Prism">
                {(
                  [
                    { id: 'search', icon: Search },
                    { id: 'chat', icon: MessageSquare },
                    { id: 'documents', icon: FileText },
                    { id: 'collections', icon: LibraryBig },
                  ] as const
                ).map((item) => (
                  <button
                    key={item.id}
                    className={`main-tab ${tab === item.id ? 'active' : ''}`}
                    aria-current={tab === item.id ? 'page' : undefined}
                    onClick={() => changeTab(item.id)}
                  >
                    <item.icon size={16} />
                    {t(item.id)}
                  </button>
                ))}
              </nav>
              <div className="workspace-editor">
                <UpdateBanner blocked={needsOnboarding || documentDirty} />
                {incomingChat && documentDirty ? (
                  <div className="chat-handoff-notice" role="status">
                    {t('chatWaitingForDocument')}
                  </div>
                ) : null}
                {!needsOnboarding ? <ClipboardNotice /> : null}
                <Suspense fallback={<span className="loading-dot" />}>
                  {tab === 'search' ? (
                    <SearchView
                      recent={recent}
                      onRecent={setRecent}
                      onAddFolder={add}
                      onRecommended={chooseRecommended}
                      onSettings={() => setSettingsTab('scopes')}
                      onContextChange={contextChanged}
                      onChat={(selection) => void openSearchChat(selection)}
                      creatingChat={creatingSearchChat}
                      onCancelChat={() => {
                        tabIntent.current++;
                        if (creationRequest.current)
                          void content.cancel(creationRequest.current).catch(() => {});
                      }}
                    />
                  ) : tab === 'chat' ? (
                    <ChatView
                      initialConversationId={initialChat}
                      onSettings={() => setSettingsTab('scopes')}
                    />
                  ) : tab === 'collections' ? (
                    <CollectionsView />
                  ) : (
                    <DocumentsView onDirtyChange={setDocumentDirty} />
                  )}
                </Suspense>
              </div>
              {tasksOpen ? <IndexingTasks onClose={() => setTasksOpen(false)} /> : null}
            </main>
            <nav className="workspace-tool-bar workspace-tool-bar-right" aria-label={t('ai')}>
              <button
                type="button"
                className="workspace-tool"
                title={w('aiTool')}
                aria-label={w('aiTool')}
                onClick={() => setSettingsTab('ai')}
              >
                <BrainCircuit size={20} />
              </button>
              <span className="workspace-tool-spacer" />
              <button
                type="button"
                className={`workspace-tool ${tasksOpen ? 'active' : ''}`}
                title={w('tasks')}
                aria-label={w('tasks')}
                aria-expanded={tasksOpen}
                aria-controls="indexing-tasks"
                onClick={() => setTasksOpen((v) => !v)}
              >
                <ListChecks size={20} />
              </button>
            </nav>
          </div>
          <footer className="statusbar">
            <span>
              <i className="status-dot" />
              {runtime.native ? t('localWorkspace') : t('browserPreview')}
            </span>
            <IndexingStatus open={tasksOpen} onToggle={() => setTasksOpen((v) => !v)} />
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
      {/* Dialogs render nothing until their code arrives (a local chunk). */}
      <Suspense fallback={null}>
        {!quick && needsOnboarding ? (
          <Onboarding
            settings={settings}
            allowStartOnLogin={runtime.installKind !== 'portable'}
            loadRecommended={bridge.recommendedFolders}
            pickFolders={bridge.chooseFolders}
            onFinish={(next) => saveSettings({ ...next, onboardingVersion: 1 })}
            onSkip={() => saveSettings({ ...settings, onboardingVersion: 1 })}
          />
        ) : null}
        {settingsTab ? (
          <SettingsDialog
            initialTab={settingsTab}
            onClose={() => setSettingsTab(null)}
            allowUpdateInstall={!documentDirty}
          />
        ) : null}
        {recommended ? (
          <RecommendedDialog folders={recommended} onClose={() => setRecommended(null)} />
        ) : null}
      </Suspense>
      {state.toast ? (
        <div className="toast" role="status">
          <Check size={16} />
          <span>{state.toast}</span>
          <button className="icon-button small" aria-label={t('close')} onClick={clearToast}>
            <X size={15} />
          </button>
        </div>
      ) : null}
    </IndexActivityProvider>
  );
}
