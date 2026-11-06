import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { loadPackageNotices } from '../../scripts/package-notices.mjs';

async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}

// Check the files written by the actual setup, independently of its payload
// builder. An exe-only setup must fail even when its app hash is correct.
export async function verifyInstalledPayload(directory, expectedExecutable) {
  const record = JSON.parse(await readFile(join(directory, 'install.json'), 'utf8'));
  const notices = loadPackageNotices(expectedExecutable);
  const names = ['prism.exe', ...notices.map(({ name }) => `notices/${name}`)];
  for (const name of names) {
    assert(record.files.includes(name), `Installed file missing from ownership record: ${name}`);
    assert((await lstat(join(directory, name))).isFile(), `Expected a regular file: ${name}`);
  }
  assert.equal(await hash(join(directory, 'prism.exe')), await hash(expectedExecutable));
  for (const { name, bytes } of notices) {
    assert.deepEqual(
      await readFile(join(directory, 'notices', name)),
      bytes,
      `Changed notice: ${name}`,
    );
  }
  await assert.rejects(lstat(join(directory, 'prism.portable')), { code: 'ENOENT' });
  return names;
}
