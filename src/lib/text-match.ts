const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
function normalized(value: string) {
  return value.normalize('NFKC').toLowerCase().replace(/\s/gu, '');
}
// Keep source text intact while mapping normalized matches back to grapheme boundaries.
export function matchedSegments(text: string, query: string) {
  const terms = [...new Set(query.split(/\s+/u).map(normalized).filter(Boolean))];
  if (!terms.length) return [{ text, matched: false }];
  let projection = '';
  const positions: { start: number; end: number }[] = [];
  for (const part of graphemes.segment(text)) {
    const folded = normalized(part.segment);
    projection += folded;
    for (let i = 0; i < folded.length; i++)
      positions.push({ start: part.index, end: part.index + part.segment.length });
  }
  const ranges: { start: number; end: number }[] = [];
  for (const term of terms) {
    let cursor = 0;
    while (cursor < projection.length) {
      const index = projection.indexOf(term, cursor);
      if (index < 0) break;
      ranges.push({ start: positions[index].start, end: positions[index + term.length - 1].end });
      cursor = index + 1;
    }
  }
  ranges.sort((left, right) => left.start - right.start || left.end - right.end);
  const merged: typeof ranges = [];
  for (const range of ranges) {
    const previous = merged.at(-1);
    if (previous && range.start <= previous.end) previous.end = Math.max(previous.end, range.end);
    else merged.push({ ...range });
  }
  const segments: { text: string; matched: boolean }[] = [];
  let cursor = 0;
  for (const range of merged) {
    if (cursor < range.start)
      segments.push({ text: text.slice(cursor, range.start), matched: false });
    segments.push({ text: text.slice(range.start, range.end), matched: true });
    cursor = range.end;
  }
  if (cursor < text.length || !segments.length)
    segments.push({ text: text.slice(cursor), matched: false });
  return segments;
}
