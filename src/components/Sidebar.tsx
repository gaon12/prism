import { Files, History, Folder, Plus, Settings2 } from 'lucide-react';
import { translate } from '../lib/i18n';
import { useClient, setSession } from '../lib/store';
export function Sidebar({
  recent,
  onRecent,
  onSettings,
  onAddFolder,
}: {
  recent: boolean;
  onRecent: (value: boolean) => void;
  onSettings: () => void;
  onAddFolder: () => void;
}) {
  const { settings, session } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  return (
    <aside className="sidebar">
      <div className="rail-section">
        <h2>{t('library')}</h2>
        <button
          className={`rail-item ${!recent && !session.scopeId ? 'active' : ''}`}
          onClick={() => {
            onRecent(false);
            setSession({ scopeId: null });
          }}
        >
          <Files size={18} />
          {t('allFiles')}
        </button>
        <button className={`rail-item ${recent ? 'active' : ''}`} onClick={() => onRecent(true)}>
          <History size={18} />
          {t('recent')}
        </button>
      </div>
      <div className="rail-section folders-section">
        <div className="rail-section-heading">
          <h2>{t('searchFolders')}</h2>
          <button className="icon-button small" aria-label={t('addFolder')} onClick={onAddFolder}>
            <Plus size={17} />
          </button>
        </div>
        {settings.scopes.length ? (
          settings.scopes.map((scope) => (
            <button
              key={scope.id}
              title={scope.path}
              className={`rail-item ${!recent && session.scopeId === scope.id ? 'active' : ''}`}
              onClick={() => {
                onRecent(false);
                setSession({ scopeId: scope.id });
              }}
            >
              <Folder size={18} />
              <span className="truncate">{scope.name}</span>
              {scope.contentEnabled ? (
                <span className="scope-dot" title={t('contentScope')} />
              ) : null}
            </button>
          ))
        ) : (
          <p className="rail-empty">{t('noFolders')}</p>
        )}
      </div>
      <button className="rail-item settings-link" onClick={onSettings}>
        <Settings2 size={18} />
        {t('settings')}
      </button>
    </aside>
  );
}
