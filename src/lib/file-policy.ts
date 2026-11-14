/** Inclusion narrows an enabled folder; it never enables content or external AI access. */
export interface FileCategories {
  hidden: boolean;
  system: boolean;
  sourceCode: boolean;
  repositoryIgnored: boolean;
  archives: boolean;
}

export interface FilePolicy {
  filename: FileCategories;
  content: FileCategories;
}

function categories(input: unknown): FileCategories {
  const value = (input && typeof input === 'object' ? input : {}) as Partial<FileCategories>;
  return {
    hidden: value.hidden !== false,
    system: value.system !== false,
    sourceCode: value.sourceCode !== false,
    repositoryIgnored: value.repositoryIgnored !== false,
    archives: value.archives !== false,
  };
}

export function normalizeFilePolicy(input: unknown): FilePolicy {
  const value = (input && typeof input === 'object' ? input : {}) as Partial<FilePolicy>;
  return { filename: categories(value.filename), content: categories(value.content) };
}
