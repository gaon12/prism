// Copy the component notices that were built beside this exact executable.
// Custom Windows packagers do not automatically include Tauri bundle resources.
import { lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, sep } from 'node:path';

export const noticeNames = Object.freeze([
  'pdf-renderer.txt',
  'document-pdf.txt',
  'document-hwpx.txt',
  'office-passwords.txt',
]);

export function loadPackageNotices(executable) {
  const parent = realpathSync(dirname(executable));
  const directory = join(dirname(executable), 'notices');
  const child = relative(parent, realpathSync(directory));
  if (
    !child ||
    child === '..' ||
    child.startsWith(`..${sep}`) ||
    isAbsolute(child) ||
    !lstatSync(directory).isDirectory()
  )
    throw new Error('Component notices must be in a real directory beside the executable.');
  return noticeNames.map((name) => {
    const path = join(directory, name);
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.size === 0 || stat.size > 1024 * 1024)
      throw new Error(`Missing or invalid component notice: ${name}`);
    return { name, bytes: readFileSync(path) };
  });
}

export function writePackageNotices(payload, notices) {
  const directory = join(payload, 'notices');
  mkdirSync(directory);
  for (const { name, bytes } of notices)
    writeFileSync(join(directory, name), bytes, { flag: 'wx' });
}
