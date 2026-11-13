import type { DocumentAssemblyPlan, DocumentPlan } from '../../lib/documents';
import type { Language } from '../../lib/types';

const bytes = (value: string) => new TextEncoder().encode(value).length;
const identifier = (value: string) =>
  value.length > 0 && value.length <= 96 && !/[^A-Za-z0-9_-]/.test(value);
export const assemblyTextValid = (value: string, chars: number, maxBytes: number) =>
  Array.from(value).length <= chars &&
  bytes(value) <= maxBytes &&
  !/[\u0000-\u001f\u007f-\u009f]/u.test(value);

export function emptyAssembly(plan: DocumentPlan, language: Language): DocumentAssemblyPlan {
  return {
    version: 1,
    noticeLanguage: language,
    fields: [],
    sections: plan.sections.map((section) => ({ sectionId: section.id, elements: [] })),
  };
}

/** An explicit outline deletion removes its layout and only now-unused fields. */
export function alignAssembly(plan: DocumentPlan): DocumentPlan {
  if (!plan.assembly) return plan;
  const existing = new Map(plan.assembly.sections.map((section) => [section.sectionId, section]));
  const sections = plan.sections.map(
    (section) => existing.get(section.id) ?? { sectionId: section.id, elements: [] },
  );
  const used = new Set(
    sections.flatMap((section) =>
      section.elements.flatMap((element) =>
        element.kind === 'field' ? [element.fieldId] : element.fields,
      ),
    ),
  );
  return {
    ...plan,
    assembly: {
      ...plan.assembly,
      sections,
      fields: plan.assembly.fields.filter((f) => used.has(f.id)),
    },
  };
}

/** Mirrors the persisted contract's budgets; the Rust validator is authoritative. */
export function assemblyPlanValid(plan: DocumentPlan): boolean {
  const value = plan.assembly;
  if (!value) return true;
  if (
    value.version !== 1 ||
    !['ko', 'en', 'zh-CN', 'zh-TW', 'ja'].includes(value.noticeLanguage) ||
    !value.fields.length ||
    value.fields.length > 64 ||
    !plan.sections.length ||
    plan.sections.length > 64 ||
    value.sections.length !== plan.sections.length ||
    bytes(JSON.stringify(value)) > 32768
  )
    return false;
  const declared = new Set<string>();
  for (const field of value.fields) {
    if (
      !identifier(field.id) ||
      declared.has(field.id) ||
      !field.label.trim() ||
      !assemblyTextValid(field.label, 128, 512) ||
      !assemblyTextValid(field.instruction, 512, 2048) ||
      !['blank', 'notice'].includes(field.missingDisplay)
    )
      return false;
    declared.add(field.id);
  }
  const sectionIds = new Set<string>();
  const used = new Set<string>();
  let elements = 0;
  let references = 0;
  for (const [index, section] of value.sections.entries()) {
    if (
      section.sectionId !== plan.sections[index].id ||
      !identifier(section.sectionId) ||
      sectionIds.has(section.sectionId) ||
      !section.elements.length
    )
      return false;
    sectionIds.add(section.sectionId);
    elements += section.elements.length;
    for (const element of section.elements) {
      if (
        element.kind === 'fieldTable' &&
        (!element.fields.length ||
          element.columns.length !== 2 ||
          element.columns[0] === element.columns[1] ||
          element.columns.some((column) => !column.trim() || !assemblyTextValid(column, 128, 512)))
      )
        return false;
      const ids = element.kind === 'field' ? [element.fieldId] : element.fields;
      references += ids.length;
      for (const id of ids) {
        if (!declared.has(id)) return false;
        used.add(id);
      }
    }
  }
  return elements <= 128 && references <= 256 && used.size === declared.size;
}
