import { Pause, RefreshCw, X, ListChecks } from 'lucide-react';
import { contentIndexBusy, useIndexActivity } from '../lib/index-activity';
import { bridge, native } from '../lib/bridge';
import { content } from '../lib/content';
import { useClient, showToast, errorMessage } from '../lib/store';
import { translate } from '../lib/i18n';
import { contentText } from '../features/content/messages';
import { workspaceText } from './workspace-messages';
import { IndexProgress } from './IndexProgress';

export function IndexingStatus({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const activity = useIndexActivity();
  const { settings } = useClient();
  const t = (key: Parameters<typeof workspaceText>[1]) => workspaceText(settings.language, key);
  const busy = activity && (activity.files.scanning || contentIndexBusy(activity.content));
  const label =
    activity?.fileError || activity?.contentError
      ? t('statusError')
      : activity?.files.scanning
        ? t('collecting')
        : activity && contentIndexBusy(activity.content)
          ? t('processing')
          : t('ready');
  return (
    <button
      type="button"
      className="workspace-index-status"
      aria-label={t('indexStatus')}
      title={label}
      aria-controls="indexing-tasks"
      aria-expanded={open}
      onClick={onToggle}
    >
      <ListChecks size={14} />
      <span>{label}</span>
      {activity ? (
        <small>{new Intl.NumberFormat(settings.language).format(activity.files.count)}</small>
      ) : null}
      <IndexProgress active={Boolean(busy)} label={label} />
    </button>
  );
}

export function IndexingTasks({ onClose }: { onClose: () => void }) {
  const activity = useIndexActivity();
  const { settings } = useClient();
  const language = settings.language;
  const t = (key: Parameters<typeof workspaceText>[1]) => workspaceText(language, key);
  const global = (key: Parameters<typeof translate>[1]) => translate(language, key);
  const c = (key: Parameters<typeof contentText>[1]) => contentText(language, key);
  const number = new Intl.NumberFormat(language);
  const run = (work: () => Promise<unknown>) => {
    void work().catch((e) => showToast(errorMessage(e)));
  };
  const contentBusy = activity && contentIndexBusy(activity.content);
  return (
    <section id="indexing-tasks" className="workspace-tasks" aria-labelledby="indexing-tasks-title">
      <header>
        <h2 id="indexing-tasks-title">
          <ListChecks size={15} />
          {t('tasks')}
        </h2>
        <button
          type="button"
          className="icon-button small"
          aria-label={global('close')}
          onClick={onClose}
        >
          <X size={15} />
        </button>
      </header>
      {!activity ? (
        <p className="muted">{global('nativeOnly')}</p>
      ) : (
        <div className="workspace-task-list">
          {activity.fileError || activity.contentError ? (
            <p role="alert">{t('statusError')}</p>
          ) : null}
          <div className="workspace-task">
            <strong>{t('filenames')}</strong>
            <span>
              {activity.files.scanning ? t('collecting') : t('ready')} ·{' '}
              {number.format(activity.files.count)}
            </span>
            <button
              type="button"
              className="icon-button small"
              title={global('refresh')}
              aria-label={`${global('refresh')}: ${t('filenames')}`}
              disabled={!native || activity.files.scanning}
              onClick={() => run(bridge.refreshIndex)}
            >
              <RefreshCw size={14} />
            </button>
            <IndexProgress active={activity.files.scanning} label={t('collecting')} />
            {activity.files.scanning ? <small className="muted">{t('waiting')}</small> : null}
          </div>
          <div className="workspace-task">
            <strong>{t('documents')}</strong>
            <span>
              {c(
                contentBusy
                  ? 'indexing'
                  : activity.content.phase === 'cancelled'
                    ? 'paused'
                    : 'ready',
              )}{' '}
              · {c('indexed')} {number.format(activity.content.indexed)} · {c('discovered')}{' '}
              {number.format(activity.content.discovered)} · {c('skipped')}{' '}
              {number.format(activity.content.skipped)}
            </span>
            <button
              type="button"
              className="icon-button small"
              title={c(contentBusy ? 'pause' : 'refresh')}
              aria-label={`${c(contentBusy ? 'pause' : 'refresh')}: ${t('documents')}`}
              disabled={!native || !settings.scopes.some((scope) => scope.contentEnabled)}
              onClick={() => run(contentBusy ? content.pause : content.refresh)}
            >
              {contentBusy ? <Pause size={14} /> : <RefreshCw size={14} />}
            </button>
            <IndexProgress active={Boolean(contentBusy)} label={t('processing')} />
            {activity.content.currentPath ? (
              <small className="workspace-task-path muted" title={activity.content.currentPath}>
                {activity.content.currentPath}
              </small>
            ) : null}
          </div>
        </div>
      )}
    </section>
  );
}
