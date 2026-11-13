import { useId, useRef } from 'react';
import type {
  DocumentAssemblyElement,
  DocumentAssemblyField,
  DocumentAssemblyPlan,
  DocumentPlan,
} from '../../lib/documents';
import type { Language } from '../../lib/types';
import { assemblyText } from './assembly-messages';
import { assemblyPlanValid, assemblyTextValid, emptyAssembly } from './assembly-plan';

type Props = {
  plan: DocumentPlan;
  language: Language;
  disabled: boolean;
  onChange: (plan: DocumentPlan) => void;
};

export function AssemblyPlanControls({ plan, language, disabled, onChange }: Props) {
  const id = useId();
  const previous = useRef<DocumentAssemblyPlan | null>(null);
  const t = (key: Parameters<typeof assemblyText>[1]) => assemblyText(language, key);
  return (
    <div className="doc-assembly-controls">
      <label className="doc-check">
        <input
          type="checkbox"
          disabled={disabled}
          checked={Boolean(plan.assembly)}
          aria-describedby={`${id}-hint`}
          onChange={(event) => {
            if (event.target.checked) {
              onChange({ ...plan, assembly: previous.current ?? emptyAssembly(plan, language) });
            } else {
              previous.current = plan.assembly ?? null;
              onChange({ ...plan, assembly: null });
            }
          }}
        />
        <span>{t('enable')}</span>
      </label>
      <p className="doc-note" id={`${id}-hint`}>
        {t('hint')}
      </p>
      {plan.assembly ? (
        <>
          <label className="doc-label" htmlFor={`${id}-language`}>
            {t('language')}
          </label>
          <select
            id={`${id}-language`}
            disabled={disabled}
            value={plan.assembly.noticeLanguage}
            onChange={(event) =>
              onChange({
                ...plan,
                assembly: {
                  ...plan.assembly!,
                  noticeLanguage: event.target.value as Language,
                },
              })
            }
          >
            <option value="ko">한국어</option>
            <option value="en">English</option>
            <option value="zh-CN">简体中文</option>
            <option value="zh-TW">繁體中文</option>
            <option value="ja">日本語</option>
          </select>
          {!assemblyPlanValid(plan) ? <p className="doc-warning">{t('invalid')}</p> : null}
        </>
      ) : null}
    </div>
  );
}

function MoveButtons({
  language,
  index,
  length,
  move,
}: {
  language: Language;
  index: number;
  length: number;
  move: (offset: number) => void;
}) {
  return (
    <>
      <button className="text-button" type="button" disabled={index === 0} onClick={() => move(-1)}>
        ↑ {assemblyText(language, 'up')}
      </button>
      <button
        className="text-button"
        type="button"
        disabled={index === length - 1}
        onClick={() => move(1)}
      >
        ↓ {assemblyText(language, 'down')}
      </button>
    </>
  );
}

function FieldEditor({
  field,
  language,
  onChange,
  onRemove,
}: {
  field: DocumentAssemblyField;
  language: Language;
  onChange: (value: DocumentAssemblyField) => void;
  onRemove: () => void;
}) {
  const id = useId();
  const t = (key: Parameters<typeof assemblyText>[1]) => assemblyText(language, key);
  return (
    <div className="doc-assembly-field" role="group" aria-label={field.label || t('field')}>
      <div className="doc-assembly-inputs">
        <div>
          <label className="doc-label" htmlFor={`${id}-name`}>
            {t('label')}
          </label>
          <input
            id={`${id}-name`}
            value={field.label}
            maxLength={256}
            required
            aria-invalid={!field.label.trim() || !assemblyTextValid(field.label, 128, 512)}
            onChange={(event) => onChange({ ...field, label: event.target.value })}
          />
        </div>
        <div>
          <label className="doc-label" htmlFor={`${id}-instruction`}>
            {t('instruction')}
          </label>
          <input
            id={`${id}-instruction`}
            value={field.instruction}
            maxLength={1024}
            aria-invalid={!assemblyTextValid(field.instruction, 512, 2048)}
            onChange={(event) => onChange({ ...field, instruction: event.target.value })}
          />
        </div>
      </div>
      <div className="doc-actions">
        <label className="doc-check">
          <input
            type="checkbox"
            checked={field.missingDisplay === 'notice'}
            onChange={(event) =>
              onChange({ ...field, missingDisplay: event.target.checked ? 'notice' : 'blank' })
            }
          />
          <span>{t('notice')}</span>
        </label>
        <button className="text-button" type="button" onClick={onRemove}>
          {t('remove')}
        </button>
      </div>
    </div>
  );
}

const newField = (): DocumentAssemblyField => ({
  id: crypto.randomUUID(),
  label: '',
  instruction: '',
  missingDisplay: 'blank',
});
function moved<T>(items: T[], index: number, offset: number): T[] {
  const result = [...items];
  const target = index + offset;
  if (target >= 0 && target < result.length)
    [result[index], result[target]] = [result[target], result[index]];
  return result;
}

export function AssemblySectionEditor({
  plan,
  sectionId,
  language,
  disabled,
  onChange,
}: Props & { sectionId: string }) {
  const id = useId();
  const assembly = plan.assembly;
  const section = assembly?.sections.find((item) => item.sectionId === sectionId);
  if (!assembly || !section) return null;
  const t = (key: Parameters<typeof assemblyText>[1]) => assemblyText(language, key);
  const fields = new Map(assembly.fields.map((field) => [field.id, field]));
  const changeElements = (elements: DocumentAssemblyElement[], nextFields = assembly.fields) =>
    onChange({
      ...plan,
      assembly: {
        ...assembly,
        fields: nextFields,
        sections: assembly.sections.map((item) =>
          item.sectionId === sectionId ? { ...item, elements } : item,
        ),
      },
    });
  const changeField = (value: DocumentAssemblyField) =>
    onChange({
      ...plan,
      assembly: {
        ...assembly,
        fields: assembly.fields.map((field) => (field.id === value.id ? value : field)),
      },
    });
  const changeElement = (index: number, element: DocumentAssemblyElement) =>
    changeElements(section.elements.map((item, position) => (position === index ? element : item)));
  const removeElement = (index: number) =>
    changeElements(section.elements.filter((_, position) => position !== index));
  const canAdd = assembly.fields.length < 64;
  return (
    <fieldset className="doc-assembly-section" disabled={disabled}>
      <legend>{t('enable')}</legend>
      {section.elements.length === 0 ? <p className="doc-note">{t('empty')}</p> : null}
      {section.elements.map((element, index) => (
        <div className="doc-assembly-element" key={index}>
          {element.kind === 'field' ? (
            fields.has(element.fieldId) ? (
              <FieldEditor
                key={element.fieldId}
                field={fields.get(element.fieldId)!}
                language={language}
                onChange={changeField}
                onRemove={() => removeElement(index)}
              />
            ) : (
              <p className="doc-warning">{t('invalid')}</p>
            )
          ) : (
            <fieldset className="doc-assembly-table">
              <legend>
                {t('table')} {index + 1}
              </legend>
              <div className="doc-assembly-inputs">
                {element.columns.map((column, columnIndex) => (
                  <div key={columnIndex}>
                    <label className="doc-label" htmlFor={`${id}-${index}-${columnIndex}`}>
                      {t(columnIndex === 0 ? 'firstColumn' : 'secondColumn')}
                    </label>
                    <input
                      id={`${id}-${index}-${columnIndex}`}
                      value={column}
                      required
                      maxLength={256}
                      aria-invalid={
                        !column.trim() ||
                        !assemblyTextValid(column, 128, 512) ||
                        element.columns[0] === element.columns[1]
                      }
                      onChange={(event) =>
                        changeElement(index, {
                          ...element,
                          columns: element.columns.map((value, n) =>
                            n === columnIndex ? event.target.value : value,
                          ) as [string, string],
                        })
                      }
                    />
                  </div>
                ))}
              </div>
              {element.fields.map((fieldId, rowIndex) => (
                <div className="doc-assembly-row" key={`${fieldId}-${rowIndex}`}>
                  {fields.has(fieldId) ? (
                    <FieldEditor
                      field={fields.get(fieldId)!}
                      language={language}
                      onChange={changeField}
                      onRemove={() => {
                        const rows = element.fields.filter((_, position) => position !== rowIndex);
                        if (rows.length) changeElement(index, { ...element, fields: rows });
                        else removeElement(index);
                      }}
                    />
                  ) : (
                    <p className="doc-warning">{t('invalid')}</p>
                  )}
                  <div className="doc-actions">
                    <MoveButtons
                      language={language}
                      index={rowIndex}
                      length={element.fields.length}
                      move={(offset) =>
                        changeElement(index, {
                          ...element,
                          fields: moved(element.fields, rowIndex, offset),
                        })
                      }
                    />
                  </div>
                </div>
              ))}
              <div className="doc-actions">
                <button
                  className="text-button"
                  type="button"
                  disabled={!canAdd}
                  onClick={() => {
                    const field = newField();
                    changeElements(
                      section.elements.map((item, n) =>
                        n === index ? { ...element, fields: [...element.fields, field.id] } : item,
                      ),
                      [...assembly.fields, field],
                    );
                  }}
                >
                  {t('addRow')}
                </button>
                <button className="text-button" type="button" onClick={() => removeElement(index)}>
                  {t('remove')} {t('table')}
                </button>
              </div>
            </fieldset>
          )}
          <div className="doc-actions">
            <MoveButtons
              language={language}
              index={index}
              length={section.elements.length}
              move={(offset) => changeElements(moved(section.elements, index, offset))}
            />
          </div>
        </div>
      ))}
      <div className="doc-actions">
        <button
          className="text-button"
          type="button"
          disabled={!canAdd || section.elements.length >= 128}
          onClick={() => {
            const field = newField();
            changeElements(
              [...section.elements, { kind: 'field', fieldId: field.id }],
              [...assembly.fields, field],
            );
          }}
        >
          {t('addField')}
        </button>
        <button
          className="text-button"
          type="button"
          disabled={!canAdd || section.elements.length >= 128}
          onClick={() => {
            const field = newField();
            changeElements(
              [
                ...section.elements,
                { kind: 'fieldTable', columns: [t('field'), t('value')], fields: [field.id] },
              ],
              [...assembly.fields, field],
            );
          }}
        >
          {t('addTable')}
        </button>
      </div>
    </fieldset>
  );
}
