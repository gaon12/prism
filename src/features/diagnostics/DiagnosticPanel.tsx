import { useState } from 'react';
import { Copy, Download, ExternalLink, RefreshCw } from 'lucide-react';
import { native } from '../../lib/bridge';
import { diagnostics, diagnosticReport, MAX_DESCRIPTION_LENGTH } from '../../lib/diagnostics';
import type { DiagnosticSnapshot } from '../../lib/diagnostics';
import type { Language } from '../../lib/types';
import { diagnosticText } from './messages';
import './diagnostics.css';

export function DiagnosticPanel({ language }: { language: Language }) {
  const [snapshot, setSnapshot] = useState<DiagnosticSnapshot | null>(null);
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<'copied' | 'saved' | 'failed' | null>(null);
  const t = (key: Parameters<typeof diagnosticText>[1]) => diagnosticText(language, key);
  const report = snapshot ? diagnosticReport(snapshot, description) : '';
  const perform = async (work: () => Promise<void>) => {
    setBusy(true);
    setMessage(null);
    try {
      await work();
    } catch {
      setMessage('failed');
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="diagnostic-panel" aria-labelledby="diagnostic-title" aria-busy={busy}>
      <h3 id="diagnostic-title">{t('title')}</h3>
      <p>{t('privacy')}</p>
      {!native ? <p className="muted">{t('native')}</p> : null}
      <button
        type="button"
        className="button"
        disabled={busy || !native}
        onClick={() => void perform(async () => setSnapshot(await diagnostics.capture()))}
      >
        <RefreshCw size={15} />
        {t(snapshot ? 'refresh' : 'capture')}
      </button>
      {snapshot ? (
        <div className="diagnostic-report">
          <label htmlFor="diagnostic-description">{t('description')}</label>
          <textarea
            id="diagnostic-description"
            rows={3}
            maxLength={MAX_DESCRIPTION_LENGTH}
            value={description}
            disabled={busy}
            onChange={(event) => {
              setDescription(event.target.value);
              setMessage(null);
            }}
          />
          <label htmlFor="diagnostic-preview">{t('preview')}</label>
          <p id="diagnostic-review">{t('review')}</p>
          <textarea
            id="diagnostic-preview"
            rows={12}
            readOnly
            spellCheck={false}
            aria-describedby="diagnostic-review"
            value={report}
          />
          <div className="diagnostic-actions">
            <button
              type="button"
              className="button"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  await diagnostics.copy(report);
                  setMessage('copied');
                })
              }
            >
              <Copy size={15} />
              {t('copy')}
            </button>
            <button
              type="button"
              className="button"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  if (await diagnostics.save(report)) setMessage('saved');
                })
              }
            >
              <Download size={15} />
              {t('save')}
            </button>
          </div>
          <p>{t('public')}</p>
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => void perform(diagnostics.openIssue)}
          >
            <ExternalLink size={15} />
            {t('issue')}
          </button>
        </div>
      ) : null}
      {busy ? <p role="status">{t('busy')}</p> : null}
      {message ? <p role={message === 'failed' ? 'alert' : 'status'}>{t(message)}</p> : null}
    </section>
  );
}
