import { MessageSquareText, FolderSearch } from 'lucide-react';
import { EmptyState } from '../components/EmptyState';
import { useClient } from '../lib/store';
import { translate } from '../lib/i18n';
export function ChatView({ onSettings }: { onSettings: () => void }) {
  const { settings } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  return (
    <section className="feature-view">
      <div className="page-heading">
        <h1>{t('chat')}</h1>
        <p>{t('chatHeading')}</p>
      </div>
      <div className="feature-content">
        <EmptyState
          icon={<MessageSquareText size={68} strokeWidth={1.25} />}
          title={t('chatHeading')}
          actions={
            <button className="button" onClick={onSettings}>
              <FolderSearch size={17} />
              {t('manageScopes')}
            </button>
          }
        >
          <p>{t('chatBody')}</p>
          <span className="availability-label">{t('planned')}</span>
        </EmptyState>
      </div>
    </section>
  );
}
