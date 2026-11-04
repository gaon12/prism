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
  X,
} from 'lucide-react';
import { EmptyState } from '../components/EmptyState';
import { useClient, setSession, submitSearch, showToast, errorMessage } from '../lib/store';
import { translate } from '../lib/i18n';
import { isComposing } from '../lib/types';

export function SearchView({
  recent,
  onRecent,
  onAddFolder,
  onRecommended,
  onSettings,
}: {
  recent: boolean;
  onRecent: (value: boolean) => void;
  onAddFolder: () => void;
  onRecommended: () => void;
  onSettings: () => void;
}) {
  const { settings, session, focusTick } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const input = useRef<HTMLInputElement>(null);
  const [filterOpen, setFilterOpen] = useState(false);
  useEffect(() => {
    if (focusTick) {
      input.current?.focus();
      input.current?.select();
    }
  }, [focusTick]);
  const scopes = settings.scopes.filter(
    (scope) => scope.filenameEnabled && (!session.scopeId || scope.id === session.scopeId),
  );
  const submit = () => {
    void submitSearch().catch((error) => showToast(errorMessage(error)));
  };
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
          value={session.query}
          placeholder={t('searchPlaceholder')}
          onChange={(event) => {
            setSession({ query: event.target.value });
            onRecent(false);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !isComposing(event.nativeEvent)) submit();
          }}
        />
        {session.query ? (
          <button
            className="icon-button small"
            aria-label={t('clearHistory')}
            onClick={() => setSession({ query: '' })}
          >
            <X size={16} />
          </button>
        ) : null}
        <kbd>Enter</kbd>
      </div>
      <div className="search-toolbar">
        <div className="search-modes">
          <button className="mode active">{t('filename')}</button>
          <button className="mode" disabled title={t('nextSearch')}>
            {t('content')}
          </button>
          <button className="mode" disabled title={t('nextSearch')}>
            {t('semantic')}
          </button>
        </div>
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
      {filterOpen ? (
        <div className="filter-panel">
          <label htmlFor="extension">{t('extension')}</label>
          <input
            id="extension"
            value={session.extension}
            placeholder={t('extensionHint')}
            onChange={(event) => setSession({ extension: event.target.value.replace(/^\./, '') })}
          />
          <span className="muted">{t('searchNotConnected')}</span>
        </div>
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
            <p>{t('engineNext')}</p>
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
        <strong>{t('filenameFirst')}</strong>
        <span>{t('nextSearch')}</span>
      </div>
    </section>
  );
}
