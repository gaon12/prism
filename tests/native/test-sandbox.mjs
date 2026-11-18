// Removing a native setup test's own sandbox after it passed.
//
// Each setup test installs the ~400 MB app into a fresh folder under the OS temp folder. Kept
// after every passing run, these add up to gigabytes. A failed run keeps its folder for
// diagnosis (this is only called after success), and PRISM_KEEP_TEST_SANDBOX=1 keeps it always.
// Screenshots directly inside the folder are kept, so their printed paths stay valid.
import { readdir, rm, rmdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';

/**
 * Removes `root` (made by mkdtemp under the temp folder with one of the setup-test prefixes)
 * except its top-level .png files. Returns true when nothing but screenshots is left.
 * Never throws: a file still held by a slow process only leaves the folder behind.
 */
export async function removeTestSandbox(root) {
  if (process.env.PRISM_KEEP_TEST_SANDBOX === '1') return false;
  const folder = resolve(root);
  // Refuse anything that is not one of our own temp folders, whatever the caller passed.
  const ours = /^prism-(setup-e2e|recovery-test|setup-data-choice|setup-trust)-[A-Za-z0-9]{6}$/;
  if (dirname(folder) !== resolve(tmpdir()) || !ours.test(basename(folder))) return false;
  try {
    let kept = 0;
    for (const entry of await readdir(folder, { withFileTypes: true })) {
      if (entry.isFile() && entry.name.toLowerCase().endsWith('.png')) {
        kept++;
        continue;
      }
      await rm(join(folder, entry.name), {
        recursive: true,
        force: true,
        maxRetries: 5,
        retryDelay: 500,
      });
    }
    if (kept === 0) await rmdir(folder);
    return true;
  } catch {
    return false;
  }
}
