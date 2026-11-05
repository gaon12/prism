import type { ChatSelection } from './rag';

/** Transfer identities, never excerpts or an implicit permission to read them. */
export function searchSelection(
  query: string,
  paths: string[],
  partial = false,
): ChatSelection | null {
  const documents = [...new Set(paths)];
  return documents.length ? { query, documents, selectedDocuments: [...documents], partial } : null;
}
