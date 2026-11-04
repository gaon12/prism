import { useEffect, useRef } from 'react';
import { Search, Folder, X } from 'lucide-react';
import { Brand } from '../components/Brand';
import { bridge } from '../lib/bridge';
import {
  useClient,
  setSession,
  switchWindow,
  submitSearch,
  showToast,
  errorMessage,
} from '../lib/store';
import { translate } from '../lib/i18n';
import { isComposing } from '../lib/types';
export function QuickWindow() {
  const { settings, session, focusTick } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const input = useRef<HTMLInputElement>(null);
  const hasScopes = settings.scopes.some((scope) => scope.filenameEnabled);
  const expanded = session.query.trim().length > 0;
  useEffect(() => {
    void bridge.resizeQuick(expanded).catch((cause) => showToast(errorMessage(cause)));
  }, [expanded, settings.fontSize]);
  useEffect(() => {
    input.current?.focus();
    if (focusTick) input.current?.select();
  }, [focusTick]);
  const openMain = () => {
    void switchWindow('main').catch((cause) => showToast(errorMessage(cause)));
  };
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (isComposing(event)) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        void bridge.hideQuick().catch((cause) => showToast(errorMessage(cause)));
      }
      if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        void switchWindow('main').catch((cause) => showToast(errorMessage(cause)));
      }
    };
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  }, []);
  return (
    <div className="quick-backdrop">
      <main className="quick-panel" aria-label={t('quickSearch')}>
        <div className="quick-search-box">
          <Search size={25} />
          <input
            ref={input}
            aria-label={t('searchTerm')}
            placeholder={t('searchPlaceholder')}
            value={session.query}
            onChange={(event) => setSession({ query: event.target.value })}
            onKeyDown={(event) => {
              if (
                event.key === 'Enter' &&
                !event.ctrlKey &&
                !event.metaKey &&
                !isComposing(event.nativeEvent)
              )
                void submitSearch().catch((cause) => showToast(errorMessage(cause)));
            }}
          />
          {session.query ? (
            <button
              className="icon-button small"
              aria-label={t('close')}
              onClick={() => setSession({ query: '' })}
            >
              <X size={17} />
            </button>
          ) : null}
          <Brand />
        </div>
        {expanded ? (
          <>
            <div className="quick-meta">
              <span>{t('filenameScope')}</span>
              <button
                className="text-button"
                onClick={() =>
                  void bridge.hideQuick().catch((cause) => showToast(errorMessage(cause)))
                }
              >
                <kbd>Esc</kbd> {t('close')}
              </button>
            </div>
            <div className="quick-empty">
              <Folder size={56} strokeWidth={1.4} />
              <h1>{hasScopes ? t('searchNotConnected') : t('quickNoFolders')}</h1>
              <p>{hasScopes ? t('engineNext') : t('quickNoFoldersBody')}</p>
              <button className="button" onClick={openMain}>
                {t('openMain')}
              </button>
            </div>
            <footer className="quick-footer">
              <span>
                <kbd>↑ ↓</kbd> {t('move')}
                <kbd>Enter</kbd> {t('open')}
              </span>
              <button className="text-button" onClick={openMain}>
                <kbd>Ctrl Enter</kbd> {t('mainWindow')}
              </button>
            </footer>
          </>
        ) : null}
      </main>
    </div>
  );
}
