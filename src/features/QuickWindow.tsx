import { useEffect, useRef } from 'react';
import { Search, Folder, X, MessageSquare } from 'lucide-react';
import { Brand } from '../components/Brand';
import { bridge, native } from '../lib/bridge';
import { useClient, setSession, switchWindow, showToast, errorMessage } from '../lib/store';
import { isComposing } from '../lib/types';
import { useSearch } from '../lib/use-search';
import { FileResults, IndexSummary, UtilityResult } from './SearchResults';
import { ClipboardPanel } from './clipboard/ClipboardPanel';
import { DictionaryPanel } from './dictionary/DictionaryPanel';
import { ContentResults } from './content/ContentResults';
import { selectionForSearch } from '../lib/search-selection';
import type { ChatSelection } from '../lib/rag';

export function QuickWindow({
  onContextChange,
  onChat,
  creatingChat,
  onCancelChat,
}: {
  onContextChange: (selection: ChatSelection | null) => void;
  onChat: (selection: ChatSelection) => void;
  creatingChat: boolean;
  onCancelChat: () => void;
}) {
  const { settings, session, focusTick } = useClient();
  const search = useSearch(true);
  const { t } = search;
  const input = useRef<HTMLInputElement>(null);
  const expanded = session.query.trim().length > 0;
  const selection = selectionForSearch(search, session, settings);
  const selectionKey = JSON.stringify(selection);
  useEffect(() => {
    onContextChange(JSON.parse(selectionKey) as ChatSelection | null);
    return () => onContextChange(null);
  }, [selectionKey, onContextChange]);
  useEffect(() => {
    void bridge.resizeQuick(expanded).catch((cause) => showToast(errorMessage(cause)));
  }, [expanded, settings.fontSize]);
  useEffect(() => {
    input.current?.focus();
    if (focusTick) input.current?.select();
  }, [focusTick]);
  const openMain = () => {
    if (creatingChat) onCancelChat();
    void switchWindow('main').catch((cause) => showToast(errorMessage(cause)));
  };
  return (
    <div
      className="quick-backdrop"
      onKeyDown={(event) => {
        if (isComposing(event.nativeEvent)) return;
        if (event.key === 'Escape') {
          event.preventDefault();
          if (creatingChat) onCancelChat();
          void bridge.hideQuick().catch((cause) => showToast(errorMessage(cause)));
        }
        if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
          event.preventDefault();
          if (event.shiftKey) {
            if (native && selection && !creatingChat) onChat(selection);
          } else openMain();
        }
      }}
    >
      <main className={`quick-panel ${expanded ? 'expanded' : ''}`} aria-label={t('quickSearch')}>
        <div className="quick-search-box">
          <Search size={25} />
          <input
            ref={input}
            aria-label={t('searchTerm')}
            aria-controls={
              search.request.kind === 'files' && search.bodyActive
                ? search.body.data?.hits.length
                  ? search.body.listId
                  : undefined
                : search.request.kind === 'clipboard'
                  ? search.clips.listId
                  : search.data?.total
                    ? search.listId
                    : undefined
            }
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
                {search.request.kind === 'files'
                  ? session.mode === 'auto'
                    ? t('autoSearch')
                    : session.mode === 'filename'
                      ? t('filenameScope')
                      : t(session.mode)
                  : t(search.request.kind)}
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
            {search.request.kind === 'files' && search.bodyActive ? (
              <div className="quick-results">
                {session.mode === 'auto' ? <IndexSummary search={search} /> : null}
                <ContentResults
                  controller={search.body}
                  compact
                  onSettings={() =>
                    void bridge
                      .openSettings('scopes')
                      .catch((cause) => showToast(errorMessage(cause)))
                  }
                  onModels={() =>
                    void bridge.openSettings('ai').catch((cause) => showToast(errorMessage(cause)))
                  }
                />
              </div>
            ) : search.request.kind === 'dictionary' ? (
              <div className="quick-results">
                <DictionaryPanel controller={search.dict} compact />
              </div>
            ) : search.request.kind === 'clipboard' ? (
              <div className="quick-results">
                <ClipboardPanel controller={search.clips} compact />
              </div>
            ) : search.request.kind !== 'files' ? (
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
                {search.request.kind === 'files' || search.request.kind === 'clipboard' ? (
                  <>
                    <kbd>↑ ↓</kbd> {t('move')}
                  </>
                ) : null}
                <kbd>Enter</kbd>{' '}
                {t(
                  search.request.kind === 'calculator' || search.request.kind === 'units'
                    ? 'copy'
                    : search.request.kind === 'clipboard'
                      ? 'paste'
                      : search.request.kind === 'dictionary'
                        ? 'lookup'
                        : search.request.kind === 'web'
                          ? 'web'
                          : 'open',
                )}
              </span>
              {search.request.kind === 'files' ? (
                <button
                  className="text-button"
                  disabled={!native || creatingChat || !selection}
                  onClick={() => {
                    if (selection) onChat(selection);
                  }}
                  title="Ctrl Shift Enter"
                >
                  <MessageSquare size={14} />
                  {t(creatingChat ? 'creatingSearchChat' : 'chatWithResults')}
                </button>
              ) : null}
              {creatingChat ? (
                <button className="text-button" onClick={onCancelChat}>
                  {t('cancel')}
                </button>
              ) : null}
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
