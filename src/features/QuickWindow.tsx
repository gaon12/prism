import { useEffect, useRef } from 'react';
import { Search, Folder, X } from 'lucide-react';
import { Brand } from '../components/Brand';
import { bridge } from '../lib/bridge';
import { useClient, setSession, switchWindow, showToast, errorMessage } from '../lib/store';
import { isComposing } from '../lib/types';
import { useSearch } from '../lib/use-search';
import { FileResults, IndexSummary, UtilityResult } from './SearchResults';

export function QuickWindow() {
  const { settings, session, focusTick } = useClient();
  const search = useSearch(true);
  const { t } = search;
  const input = useRef<HTMLInputElement>(null);
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
      <main className={`quick-panel ${expanded ? 'expanded' : ''}`} aria-label={t('quickSearch')}>
        <div className="quick-search-box">
          <Search size={25} />
          <input
            ref={input}
            aria-label={t('searchTerm')}
            aria-controls={search.data?.total ? search.listId : undefined}
            aria-activedescendant={search.selectedId}
            aria-autocomplete="list"
            placeholder={t('quickPlaceholder')}
            value={session.query}
            onChange={(event) => setSession({ query: event.target.value })}
            onKeyDown={search.keyDown}
          />
          {session.query ? (
            <button
              className="icon-button small"
              aria-label={t('clearQuery')}
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
              <span>
                {search.request.kind === 'files' ? t('filenameScope') : t(search.request.kind)}
              </span>
              <button
                className="text-button"
                onClick={() =>
                  void bridge.hideQuick().catch((cause) => showToast(errorMessage(cause)))
                }
              >
                <kbd>Esc</kbd> {t('close')}
              </button>
            </div>
            {search.request.kind !== 'files' ? (
              <div className="quick-results">
                <UtilityResult search={search} />
              </div>
            ) : search.scopes.length ? (
              <div className="quick-results">
                <IndexSummary search={search} />
                <FileResults search={search} quick />
              </div>
            ) : (
              <div className="quick-empty">
                <Folder size={56} strokeWidth={1.4} />
                <h1>{t('quickNoFolders')}</h1>
                <p>{t('quickNoFoldersBody')}</p>
                <button className="button" onClick={openMain}>
                  {t('openMain')}
                </button>
              </div>
            )}
            <footer className="quick-footer">
              <span>
                {search.request.kind === 'files' ? (
                  <>
                    <kbd>↑ ↓</kbd> {t('move')}
                  </>
                ) : null}
                <kbd>Enter</kbd>{' '}
                {t(
                  search.request.kind === 'calculator' || search.request.kind === 'units'
                    ? 'copy'
                    : search.request.kind === 'web'
                      ? 'web'
                      : 'open',
                )}
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
