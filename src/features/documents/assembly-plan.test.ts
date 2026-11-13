import { describe, expect, it } from 'vitest';
import type { DocumentPlan } from '../../lib/documents';
import {
  alignAssembly,
  assemblyPlanValid,
  assemblyTextValid,
  emptyAssembly,
} from './assembly-plan';

const plan = (): DocumentPlan => ({
  title: 'Report',
  sections: [
    { id: 'first', heading: 'Overview', instruction: '' },
    { id: 'second', heading: 'Details', instruction: '' },
  ],
  assembly: {
    version: 1,
    noticeLanguage: 'en',
    fields: [
      { id: 'a', label: 'Alpha', instruction: '', missingDisplay: 'blank' },
      { id: 'b', label: 'Beta', instruction: '', missingDisplay: 'notice' },
    ],
    sections: [
      { sectionId: 'first', elements: [{ kind: 'field', fieldId: 'a' }] },
      {
        sectionId: 'second',
        elements: [{ kind: 'fieldTable', columns: ['Field', 'Value'], fields: ['b'] }],
      },
    ],
  },
});

describe('reviewed field layout editing', () => {
  it('keeps legacy plans untouched and requires completing a newly selected layout', () => {
    const legacy = { title: 'Report', sections: plan().sections };
    expect(alignAssembly(legacy)).toBe(legacy);
    expect(assemblyPlanValid(legacy)).toBe(true);
    const assembly = emptyAssembly(legacy, 'ja');
    expect(assembly.noticeLanguage).toBe('ja');
    expect(assembly.fields).toEqual([]);
    expect(assemblyPlanValid({ ...legacy, assembly })).toBe(false);
  });

  it('removes only deleted sections and unused fields without mutating the prior revision', () => {
    const previous = plan();
    const before = JSON.stringify(previous);
    const next = alignAssembly({ ...previous, sections: [previous.sections[1]] });
    expect(next.assembly?.sections.map((s) => s.sectionId)).toEqual(['second']);
    expect(next.assembly?.fields.map((f) => f.id)).toEqual(['b']);
    expect(assemblyPlanValid(next)).toBe(true);
    expect(JSON.stringify(previous)).toBe(before);
    const reordered = alignAssembly({ ...previous, sections: [...previous.sections].reverse() });
    expect(reordered.assembly?.sections.map((s) => s.sectionId)).toEqual(['second', 'first']);
    expect(assemblyPlanValid(reordered)).toBe(true);
    const appended = alignAssembly({
      ...previous,
      sections: [...previous.sections, { id: 'third', heading: 'More', instruction: '' }],
    });
    expect(appended.assembly?.sections[2].elements).toEqual([]);
    expect(assemblyPlanValid(appended)).toBe(false);
  });

  it('retains a shared field when one reference site is removed', () => {
    const previous = plan();
    previous.assembly!.sections[1].elements.push({ kind: 'field', fieldId: 'a' });
    const next = alignAssembly({ ...previous, sections: [previous.sections[1]] });
    expect(next.assembly?.fields.map((f) => f.id)).toEqual(['a', 'b']);
    expect(assemblyPlanValid(next)).toBe(true);
  });

  it('checks Unicode scalar and UTF-8 limits consistently with the saved contract', () => {
    expect(assemblyTextValid('🙂'.repeat(128), 128, 512)).toBe(true);
    expect(assemblyTextValid('🙂'.repeat(129), 128, 512)).toBe(false);
    expect(assemblyTextValid('value\n', 128, 512)).toBe(false);
    expect(assemblyTextValid('value\u0085', 128, 512)).toBe(false);
    const value = plan();
    value.assembly!.fields[0].id = 'a\n';
    expect(assemblyPlanValid(value)).toBe(false);
  });

  it('prevents saving dangling or duplicate fields and incomplete table headings', () => {
    expect(assemblyPlanValid(plan())).toBe(true);
    const variants = [plan(), plan(), plan(), plan(), plan()];
    variants[0].assembly!.fields.push({ ...variants[0].assembly!.fields[0] });
    variants[1].assembly!.fields[1].id = 'unreferenced';
    const table = variants[2].assembly!.sections[1].elements[0];
    if (table.kind === 'fieldTable') table.columns = ['Field', 'Field'];
    variants[3].assembly!.fields[0].label = ' ';
    variants[4].assembly!.fields.push(
      ...Array.from({ length: 64 }, (_, n) => ({
        id: `extra_${n}`,
        label: 'Extra',
        instruction: '',
        missingDisplay: 'blank' as const,
      })),
    );
    for (const value of variants) expect(assemblyPlanValid(value)).toBe(false);
  });
});
