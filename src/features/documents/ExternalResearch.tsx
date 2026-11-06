import { useEffect, useId, useState } from 'react';
import { documents } from '../../lib/documents';
import type { DocumentExternalSource, DocumentRevision } from '../../lib/documents';
import type { Language } from '../../lib/types';
import { researchOutcome, researchText } from './research-messages';

export const validResearchQuery = (query: string) =>
  Boolean(query.trim()) && new TextEncoder().encode(query).length <= 4096 && !/\p{Cc}/u.test(query);

export function ExternalResearchInput({
  language,
  available,
  busy,
  enabled,
  query,
  onEnabled,
  onQuery,
}: {
  language: Language;
  available: boolean;
  busy: boolean;
  enabled: boolean;
  query: string;
  onEnabled: (value: boolean) => void;
  onQuery: (value: string) => void;
}) {
  const id = useId();
  const t = (key: Parameters<typeof researchText>[1]) => researchText(language, key);
  return (
    <fieldset disabled={busy} className="doc-research-input">
      <label className="doc-check">
        <input
          type="checkbox"
          checked={enabled && available}
          disabled={!available}
          onChange={(event) => onEnabled(event.target.checked)}
        />
        <span>{t('optIn')}</span>
      </label>
      {available ? (
        <>
          <label className="doc-label" htmlFor={id}>
            {t('query')}
          </label>
          <input
            id={id}
            type="text"
            value={query}
            disabled={!enabled}
            maxLength={4096}
            onChange={(event) => onQuery(event.target.value)}
            aria-invalid={enabled && !validResearchQuery(query)}
          />
          <p className="doc-note">{t('help')}</p>
          {enabled && !validResearchQuery(query) ? (
            <p className="doc-warning">{t('invalid')}</p>
          ) : null}
        </>
      ) : (
        <p className="doc-note">{t('configure')}</p>
      )}
    </fieldset>
  );
}

export function DocumentResearchSources({
  revision,
  policyKey,
  language,
}: {
  revision: DocumentRevision;
  policyKey: string;
  language: Language;
}) {
  const key = JSON.stringify([revision.id, policyKey]);
  const [result, setResult] = useState<{
    key: string;
    items: DocumentExternalSource[];
    error: boolean;
  } | null>(null);
  useEffect(() => {
    let active = true;
    void documents
      .externalSources(revision.documentId, revision.id)
      .then((items) => {
        if (active) setResult({ key, items, error: false });
      })
      .catch(() => {
        if (active) setResult({ key, items: [], error: true });
      });
    return () => {
      active = false;
    };
  }, [key, revision.documentId, revision.id]);
  if (result?.key !== key || (!result.error && !result.items.length)) return null;
  const t = (name: Parameters<typeof researchText>[1]) => researchText(language, name);
  return (
    <section className="doc-research-sources" aria-label={t('heading')}>
      <h4>{t('heading')}</h4>
      <p className="doc-note">{t('untrusted')}</p>
      {result.error ? (
        <p role="alert">{t('unavailable')}</p>
      ) : (
        <ul>
          {result.items.map((item, position) => {
            const citation =
              item.source &&
              revision.citations.find(
                (cite) =>
                  cite.source.chunkId === item.source?.chunkId &&
                  cite.source.sha256 === item.source?.sha256,
              );
            return (
              <li key={`${item.source?.chunkId ?? item.evidence.serverId}:${position}`}>
                <strong>
                  {citation ? `[${citation.label}] ` : ''}
                  {item.evidence.serverName} · {item.evidence.tool}
                </strong>
                <p>{researchOutcome(language, item.evidence.outcome)}</p>
                {item.evidence.text ? (
                  <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
                    {item.evidence.text}
                  </pre>
                ) : null}
                {item.evidence.truncated ? <small>{t('truncated')}</small> : null}
                <small style={{ overflowWrap: 'anywhere' }}>
                  {t('fingerprint')}: {item.evidence.fingerprint}
                </small>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
