/** Explicit file syntax stays on the filename path, where all filters apply. */
export function automaticBodyEligible(query: string) {
  const value = query.trim();
  if (!value) return false;
  // Match the filename parser's NFKC normalization and quote removal. Holding
  // quoted file-looking syntax is conservative: never drop a filter on fallback.
  const path = value.normalize('NFKC').replaceAll('"', '');
  if (/(?:^|\s)(?:name|path|ext|extension|edit-date):/iu.test(path)) return false;
  return !path.includes('/') && !path.includes('\\') && !/^[a-z]:(?:$|[\\/])/iu.test(path);
}
