import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { isAbsolute, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { noticeNames } from '../../scripts/package-notices.mjs';

const workspace = realpathSync(fileURLToPath(new URL('../../', import.meta.url)));

export function packagingDirectory(t, label) {
  const parent = join(workspace, '.dev');
  mkdirSync(parent, { recursive: true });
  const actualParent = realpathSync(parent);
  const relativeParent = relative(workspace, actualParent);
  if (
    !relativeParent ||
    relativeParent === '..' ||
    relativeParent.startsWith(`..${sep}`) ||
    isAbsolute(relativeParent)
  )
    throw new Error('Packaging test scratch must stay in this workspace.');
  const directory = mkdtempSync(join(actualParent, `${label}-`));
  const owned = realpathSync(directory);
  t.after(() => {
    const actual = realpathSync(directory);
    const child = relative(actualParent, actual);
    if (
      actual !== owned ||
      !child ||
      child === '..' ||
      child.startsWith(`..${sep}`) ||
      isAbsolute(child)
    )
      throw new Error('Refusing packaging fixture cleanup outside its owned directory.');
    rmSync(actual, { recursive: true, force: true });
  });
  return directory;
}

export function seedNotices(executableDirectory) {
  const directory = join(executableDirectory, 'notices');
  mkdirSync(directory, { recursive: true });
  for (const name of noticeNames)
    copyFileSync(join(workspace, 'src-tauri/notices', name), join(directory, name));
}
