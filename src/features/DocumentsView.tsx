import { useEffect, useState } from 'react';
import { Download, FileText, Check, AlertCircle } from 'lucide-react';
import { translate } from '../lib/i18n';
import { bridge } from '../lib/bridge';
import { useClient, showToast, errorMessage } from '../lib/store';
interface Draft {
  title: string;
  body: string;
}
const draftKey = 'prism.draft.v1';
function readDraft(): Draft {
  try {
    const value = JSON.parse(localStorage.getItem(draftKey) ?? '{}') as Partial<Draft>;
    return {
      title: typeof value.title === 'string' ? value.title : '',
      body: typeof value.body === 'string' ? value.body : '',
    };
  } catch {
    return { title: '', body: '' };
  }
}
export function DocumentsView() {
  const { settings } = useClient();
  const t = (key: Parameters<typeof translate>[1]) => translate(settings.language, key);
  const [draft, setDraft] = useState(readDraft);
  const [saving, setSaving] = useState(false);
  const [draftSaved, setDraftSaved] = useState(true);
  useEffect(() => {
    try {
      localStorage.setItem(draftKey, JSON.stringify(draft));
      setDraftSaved(true);
    } catch {
      setDraftSaved(false);
      showToast(translate(settings.language, 'draftError'));
    }
  }, [draft, settings.language]);
  const exportDraft = async () => {
    setSaving(true);
    try {
      if (await bridge.exportMarkdown(draft.title || t('untitled'), draft.body))
        showToast(t('exported'));
    } catch (error) {
      showToast(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };
  return (
    <section className="documents-view">
      <div className="page-heading documents-heading">
        <div>
          <h1>{t('documentHeading')}</h1>
          <p>{t('documentSubtitle')}</p>
        </div>
        <button className="button" onClick={() => void exportDraft()} disabled={saving}>
          <Download size={16} />
          {t('exportMarkdown')}
        </button>
      </div>
      <div className="editor-shell">
        <div className="editor-toolbar">
          <span>
            <FileText size={16} />
            {t('draft')}
          </span>
          <span className="muted">
            {draftSaved ? <Check size={14} /> : <AlertCircle size={14} />}
            {t(draftSaved ? 'draftSaved' : 'draftError')}
          </span>
        </div>
        <input
          className="document-title"
          aria-label={t('documentTitle')}
          placeholder={t('untitled')}
          value={draft.title}
          onChange={(event) => setDraft({ ...draft, title: event.target.value })}
        />
        <textarea
          className="markdown-editor"
          aria-label="Markdown"
          spellCheck={false}
          value={draft.body}
          placeholder={t('editorPlaceholder')}
          onChange={(event) => setDraft({ ...draft, body: event.target.value })}
        />
      </div>
      <p className="document-note">{t('aiDocumentLater')}</p>
    </section>
  );
}
