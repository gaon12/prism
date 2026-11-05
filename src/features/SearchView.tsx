import { useEffect, useRef, useState } from 'react';
import {
  Search,
  FolderSearch,
  FolderCheck,
  Plus,
  Filter,
  ChevronDown,
  FileText,
  History,
  MessageSquare,
  X,
} from 'lucide-react';
import { EmptyState } from '../components/EmptyState';
import { Segmented } from '../components/controls/Segmented';
import type { SearchSession } from '../lib/types';
import { useClient, setSession } from '../lib/store';
import { translate } from '../lib/i18n';
import { useSearch } from '../lib/use-search';
import { FileResults, IndexSummary, UtilityResult } from './SearchResults';
import { bridge, native } from '../lib/bridge';
import { ContentResults } from './content/ContentResults';
import { errorMessage, showToast } from '../lib/store';
import { ClipboardPanel } from './clipboard/ClipboardPanel';
import { DictionaryPanel } from './dictionary/DictionaryPanel';
import { useChatSelection } from '../lib/use-chat-selection';
import type { ChatSelection } from '../lib/rag';

export function SearchView({
  recent,
  onRecent,
  onAddFolder,
  onRecommended,
  onSettings,
  onContextChange,
  onChat,
  creatingChat,
  onCancelChat,
}: {
  recent: boolean;
  onRecent: (value: boolean) => void;
  onAddFolder: () => void;
  onRecommended: () => void;
  onSettings: () => void;
  onContextChange: (selection: ChatSelection | null) => void;
  onChat: (selection: ChatSelection) => void;
  creatingChat: boolean;
  onCancelChat: () => void;
}) {
  const { settings, session, focusTick } = useClient();
  const search = useSearch();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const input = useRef<HTMLInputElement>(null);
  const [filterOpen, setFilterOpen] = useState(false);
  // Steady while indexing: no flicker between parts (see use-chat-selection.ts).
  const { selection, waitingForIndex } = useChatSelection(search, session, settings);
  const selectionKey = JSON.stringify(selection);
  useEffect(() => {
    onContextChange(JSON.parse(selectionKey) as ChatSelection | null);
    return () => onContextChange(null);
  }, [selectionKey, onContextChange]);
  useEffect(() => {
    if (focusTick) {
      input.current?.focus();
      input.current?.select();
    }
  }, [focusTick]);
  const scopes = settings.scopes.filter(
    (scope) => scope.filenameEnabled && (!session.scopeId || scope.id === session.scopeId),
  );
  return (
    <section className="search-view">
      <div className="page-heading">
        <h1>{recent ? t('recent') : t('fileSearch')}</h1>
        <p>{t('searchSubtitle')}</p>
      </div>
      <div className="search-box">
        <Search size={22} />
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
          value={session.query}
          placeholder={t('searchPlaceholder')}
          onChange={(event) => {
            setSession({ query: event.target.value });
            onRecent(false);
          }}
          onKeyDown={search.keyDown}
        />
        {session.query ? (
          <button
            className="icon-button small"
            aria-label={t('clearQuery')}
            onClick={() => setSession({ query: '' })}
          >
            <X size={16} />
          </button>
        ) : null}
        <kbd>Enter</kbd>
      </div>
      <div className="search-toolbar">
        <Segmented<SearchSession['mode']>
          className="search-modes"
          label={t('searchMode')}
          value={search.request.kind === 'files' ? session.mode : null}
          items={[
            { value: 'auto', label: t('autoSearch'), description: t('automaticSearchHint') },
            { value: 'filename', label: t('filename') },
            { value: 'content', label: t('content') },
            { value: 'semantic', label: t('semantic') },
          ]}
          onChange={(mode) =>
            setSession({
              mode,
              ...(search.request.kind !== 'files' ? { query: search.request.input } : {}),
            })
          }
        />
        <button
          className={`button filter-button ${filterOpen ? 'selected' : ''}`}
          aria-expanded={filterOpen}
          onClick={() => setFilterOpen((value) => !value)}
        >
          <Filter size={16} />
          {t('filter')}
          <ChevronDown size={13} />
          {session.extension ? <span className="filter-dot" /> : null}
        </button>
      </div>
      {search.request.kind === 'files' ? (
        <div className="search-chat-action">
          <button
            type="button"
            className="button"
            disabled={!native || creatingChat || !selection}
            onClick={() => {
              if (selection) onChat(selection);
            }}
          >
            <MessageSquare size={16} />
            {t(creatingChat ? 'creatingSearchChat' : 'chatWithResults')}
          </button>
          {creatingChat ? (
            <button type="button" className="text-button" onClick={onCancelChat}>
              {t('cancel')}
            </button>
          ) : null}
          <small className="muted">
            {waitingForIndex
              ? t('chatWaitsForIndex')
              : !selection || selection.filenameSearch || selection.keywordSearch
                ? t('allFilenameChatHint')
                : t('searchChatHint').replace('{count}', String(selection.documents.length))}
          </small>
        </div>
      ) : null}
      {filterOpen ? (
        <div className="filter-panel">
          <label htmlFor="extension">{t('extension')}</label>
          <input
            id="extension"
            value={session.extension}
            placeholder={t('extensionHint')}
            onChange={(event) => setSession({ extension: event.target.value.replace(/^\./, '') })}
          />
          <span className="muted">
            {t(!search.bodyActive ? 'filenameSyntax' : 'contentSyntax')}
          </span>
        </div>
      ) : null}
      <IndexSummary search={search} />
      {search.request.kind === 'files' &&
      (!search.bodyActive || session.mode === 'auto') &&
      search.status.warnings.length ? (
        <details className="index-warning">
          <summary>
            {search.status.warnings.length} · {t('searchFolders')}
          </summary>
          {search.status.warnings.map((warning) => (
            <p key={warning}>{warning}</p>
          ))}
        </details>
      ) : null}
      <div className="search-content">
        {recent ? (
          session.recentQueries.length ? (
            <div className="recent-list">
              <div className="list-heading">
                <span>{t('recent')}</span>
                <button className="text-button" onClick={() => setSession({ recentQueries: [] })}>
                  {t('clearHistory')}
                </button>
              </div>
              {session.recentQueries.map((query) => (
                <button
                  className="recent-row"
                  key={query}
                  onClick={() => {
                    setSession({ query });
                    onRecent(false);
                    input.current?.focus();
                  }}
                >
                  <History size={17} />
                  <span>{query}</span>
                  <span className="muted">↗</span>
                </button>
              ))}
            </div>
          ) : (
            <EmptyState icon={<History size={54} />} title={t('noRecent')}>
              <p>{t('searchPlaceholder')}</p>
            </EmptyState>
          )
        ) : search.request.kind === 'files' && search.bodyActive ? (
          <ContentResults
            controller={search.body}
            onSettings={() =>
              void bridge.openSettings('scopes').catch((cause) => showToast(errorMessage(cause)))
            }
            onModels={() =>
              void bridge.openSettings('ai').catch((cause) => showToast(errorMessage(cause)))
            }
          />
        ) : search.request.kind === 'dictionary' ? (
          <DictionaryPanel controller={search.dict} />
        ) : search.request.kind === 'clipboard' ? (
          <ClipboardPanel controller={search.clips} />
        ) : search.request.kind !== 'files' ? (
          <UtilityResult search={search} />
        ) : session.mode === 'auto' &&
          search.body.scopes.length &&
          !scopes.length &&
          !session.query.trim() ? (
          <EmptyState icon={<FileText size={54} />} title={t('typeToSearch')}>
            <p>{t('automaticSearchHint')}</p>
          </EmptyState>
        ) : scopes.length && (native || session.query.trim()) ? (
          <FileResults search={search} />
        ) : scopes.length ? (
          <EmptyState
            icon={<FolderCheck size={66} strokeWidth={1.3} />}
            title={t('scopeReady')}
            actions={
              <button className="button" onClick={onSettings}>
                {t('manageScopes')}
              </button>
            }
          >
            <p>{t('typeToSearch')}</p>
            <p>{t('filenameSyntax')}</p>
            <div className="registered-scopes">
              {scopes.map((scope) => (
                <span key={scope.id} title={scope.path}>
                  {scope.name}
                </span>
              ))}
            </div>
          </EmptyState>
        ) : (
          <EmptyState
            icon={<FolderSearch size={76} strokeWidth={1.25} />}
            title={t('addFoldersHeading')}
            actions={
              <>
                <button className="button primary" onClick={onAddFolder}>
                  <Plus size={17} />
                  {t('addFolder')}
                </button>
                <button className="button" onClick={onRecommended}>
                  {t('recommended')}
                </button>
              </>
            }
          >
            <p>{t('addFoldersBody')}</p>
            <p>{t('scopeHint')}</p>
          </EmptyState>
        )}
      </div>
      <div className="phase-note">
        <FileText size={19} />
        <strong>
          {t('calculator')} · {t('units')} · {t('web')}
        </strong>
        <span>
          <code>= 2+3</code> · <code>unit: 10 inch to cm</code> · <code>web: Prism</code>
        </span>
        <button
          className="text-button"
          onClick={() => {
            setSession({ query: 'dict:' });
            onRecent(false);
            input.current?.focus();
          }}
        >
          {t('dictionary')}
        </button>
        <button
          className="text-button"
          onClick={() => {
            setSession({ query: 'clip:' });
            onRecent(false);
            input.current?.focus();
          }}
        >
          {t('clipboard')}
        </button>
      </div>
    </section>
  );
}
