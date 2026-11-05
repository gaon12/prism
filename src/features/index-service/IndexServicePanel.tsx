import { useCallback, useEffect, useRef, useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';
import { indexService, indexServiceIssue } from '../../lib/index-service';
import type {
  IndexServiceAction,
  IndexServiceIssue,
  IndexServiceStatus,
} from '../../lib/index-service';
import type { Language, RuntimeInfo } from '../../lib/types';
import { indexServiceText } from './messages';
import './index-service.css';

export function IndexServicePanel({
  language,
  runtime,
}: {
  language: Language;
  runtime: RuntimeInfo;
}) {
  if (!runtime.native || runtime.platform !== 'windows') return null;
  return <WindowsServicePanel language={language} />;
}
function WindowsServicePanel({ language }: { language: Language }) {
  const [status, setStatus] = useState<IndexServiceStatus | null>(null);
  const [error, setError] = useState<IndexServiceIssue | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState<IndexServiceAction | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const mounted = useRef(false);
  const epoch = useRef(0);
  const operating = useRef(false);
  const t = (key: Parameters<typeof indexServiceText>[1]) => indexServiceText(language, key);
  const refresh = useCallback(async () => {
    const request = ++epoch.current;
    setLoading(true);
    try {
      const next = await indexService.status();
      if (mounted.current && request === epoch.current) setStatus(next);
    } catch (cause) {
      if (mounted.current && request === epoch.current) setError(indexServiceIssue(cause));
    } finally {
      if (mounted.current && request === epoch.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
      epoch.current++;
    };
  }, [refresh]);
  // Reopening settings while another window owns an action follows that action
  // until it ends. Ordinary idle status is not continuously polled.
  useEffect(() => {
    if (!status?.busy || pending) return;
    const timer = setTimeout(() => void refresh(), 800);
    return () => clearTimeout(timer);
  }, [status, pending, refresh]);
  const act = async (action: IndexServiceAction) => {
    if (operating.current) return;
    operating.current = true;
    epoch.current++;
    setConfirmRemove(false);
    setError(null);
    setPending(action);
    try {
      const next = await indexService.manage(action);
      if (mounted.current) setStatus(next);
    } catch (cause) {
      if (mounted.current) {
        setError(indexServiceIssue(cause));
        await refresh();
      }
    } finally {
      operating.current = false;
      if (mounted.current) {
        setPending(null);
        setLoading(false);
      }
    }
  };
  const working = Boolean(pending || status?.busy);
  const blocked = loading || working;
  return (
    <section
      className="index-service-panel"
      aria-labelledby="index-service-title"
      aria-busy={working}
    >
      <div className="index-service-heading">
        <h3 id="index-service-title">
          <ShieldCheck size={16} />
          {t('title')}
        </h3>
        <button
          type="button"
          className="icon-button small"
          disabled={blocked}
          aria-label={t('refresh')}
          onClick={() => {
            setError(null);
            void refresh();
          }}
        >
          <RefreshCw size={15} />
        </button>
      </div>
      <p className="setting-description">{t('description')}</p>
      <p className="index-service-state" role="status">
        {working
          ? t('working')
          : status
            ? t(status.state === 'unsupported' ? 'unsupported' : status.state)
            : t(loading ? 'checking' : 'unavailable')}
      </p>
      {working ? <progress aria-label={t('working')} /> : null}
      {status?.issue ? <p className="index-service-note">{t(status.issue)}</p> : null}
      {status && !status.managementAllowed ? (
        <p className="index-service-note">{t('isolated')}</p>
      ) : null}
      {error ? (
        <p
          role={error === 'cancelled' ? 'status' : 'alert'}
          className={error === 'cancelled' ? 'muted' : 'inline-error'}
        >
          {t(error)}
        </p>
      ) : null}
      <div className="index-service-actions">
        {!status || status.state === 'notInstalled' ? (
          <button
            type="button"
            className="button compact"
            disabled={blocked || !status?.canInstall}
            onClick={() => void act('install')}
          >
            {t('install')}
          </button>
        ) : null}
        {status?.state === 'stopped' ? (
          <button
            type="button"
            className="button compact"
            disabled={blocked || !status.canStart}
            onClick={() => void act('start')}
          >
            {t('start')}
          </button>
        ) : null}
        {status && ['running', 'stopped'].includes(status.state) ? (
          <button
            type="button"
            className="button compact"
            disabled={blocked || !status.canUpgrade}
            onClick={() => void act('upgrade')}
          >
            {t('upgrade')}
          </button>
        ) : null}
        {status && ['running', 'stopped'].includes(status.state) ? (
          <button
            type="button"
            className="button compact"
            disabled={blocked || !status.canRemove}
            onClick={() => setConfirmRemove(true)}
          >
            {t('uninstall')}
          </button>
        ) : null}
      </div>
      {status && ['running', 'stopped'].includes(status.state) ? (
        <p className="index-service-note">{t('upgradeBody')}</p>
      ) : null}
      {confirmRemove ? (
        <div className="index-service-confirm" role="group" aria-label={t('removeQuestion')}>
          <strong>{t('removeQuestion')}</strong>
          <p>{t('removeBody')}</p>
          <div className="index-service-actions">
            <button
              type="button"
              className="button compact"
              onClick={() => setConfirmRemove(false)}
            >
              {t('cancel')}
            </button>
            <button
              type="button"
              className="button compact"
              disabled={blocked || !status?.canRemove}
              onClick={() => void act('uninstall')}
            >
              {t('confirmRemove')}
            </button>
          </div>
        </div>
      ) : null}
      <p className="index-service-note">{t('consent')}</p>
      <p className="index-service-note">{t('immediate')}</p>
      <p className="index-service-note">{t('fallback')}</p>
    </section>
  );
}
