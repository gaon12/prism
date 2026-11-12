// Compare an inspected table's header with the user's requested column names.
// This is an assertion helper only; it never changes a generated document.
export function hasTableHeaders(table, expected) {
  if (!Array.isArray(table) || !Array.isArray(table[0]) || expected.length === 0) return false;
  const normalize = (value) => value.normalize('NFC').replace(/\s+/gu, '');
  return (
    table[0].length === expected.length &&
    table[0].every(
      (cell, index) => typeof cell === 'string' && normalize(cell) === normalize(expected[index]),
    )
  );
}
