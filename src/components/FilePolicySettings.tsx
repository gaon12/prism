import { translate } from '../lib/i18n';
import type { FileCategories, FilePolicy } from '../lib/file-policy';
import type { Language } from '../lib/types';

const categories = [
  ['hidden', 'filePolicyHidden'],
  ['system', 'filePolicySystem'],
  ['sourceCode', 'filePolicySourceCode'],
  ['repositoryIgnored', 'filePolicyRepositoryIgnored'],
  ['archives', 'filePolicyArchives'],
] as const;
const targets = [
  ['filename', 'filenameScope'],
  ['content', 'contentScope'],
] as const;

export function FilePolicySettings({
  value,
  language,
  disabled,
  onChange,
}: {
  value: FilePolicy;
  language: Language;
  disabled: boolean;
  onChange: (target: keyof FilePolicy, category: keyof FileCategories, included: boolean) => void;
}) {
  const t = (key: Parameters<typeof translate>[1]) => translate(language, key);
  return (
    <section className="file-policy-settings" aria-labelledby="file-policy-title">
      <h3 id="file-policy-title">{t('filePolicyTitle')}</h3>
      <p className="setting-description" id="file-policy-hint">
        {t('filePolicyHint')}
      </p>
      <div className="file-policy-groups">
        {targets.map(([target, label]) => (
          <fieldset key={target} disabled={disabled} aria-describedby="file-policy-hint">
            <legend>{t(label)}</legend>
            {categories.map(([category, categoryLabel]) => (
              <label className="setting-check" key={category}>
                <span>{t(categoryLabel)}</span>
                <input
                  type="checkbox"
                  checked={value[target][category]}
                  onChange={(event) => onChange(target, category, event.currentTarget.checked)}
                />
              </label>
            ))}
          </fieldset>
        ))}
      </div>
      <p className="setting-footnote">{t('filePolicyRulesHint')}</p>
      <p className="setting-footnote">{t('filePolicyPermissionHint')}</p>
    </section>
  );
}
