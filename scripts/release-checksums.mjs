#!/usr/bin/env node
// Check the exact named download set; never upload arbitrary files from a build folder.
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { updateFiles } from './make-latest-json.mjs';

export function downloadFiles(version) {
  const updates = [...new Set(Object.values(updateFiles(version)))];
  const primary = [
    updates[0],
    `Prism_${version}_x64_portable.zip`,
    `Prism_${version}_universal.dmg`,
    `Prism_${version}_amd64.AppImage`,
  ];
  return {
    primary,
    allowed: new Set([...primary, ...updates, ...updates.map((file) => `${file}.sig`)]),
  };
}
export async function checksums({ directory, version, write = false, complete = false }) {
  const { primary, allowed } = downloadFiles(version);
  const names = await readdir(directory);
  const present = new Set(names);
  const files = [];
  for (const name of names) {
    const base = name.endsWith('.sha256') ? name.slice(0, -7) : name;
    const metadata = await lstat(join(directory, name));
    if (!allowed.has(base) || !metadata.isFile() || metadata.size === 0)
      throw new Error(`Unexpected download file/type: ${name}`);
    if (name.endsWith('.sha256')) {
      if (!present.has(base)) throw new Error(`Checksum has no download: ${name}`);
    } else {
      if (name.endsWith('.sig') && !present.has(name.slice(0, -4)))
        throw new Error(`Signature has no download: ${name}`);
      files.push(name);
    }
  }
  if (!files.length || (complete && primary.some((name) => !present.has(name))))
    throw new Error('Required release downloads are missing.');
  const pending = [];
  for (const name of files.sort()) {
    const hash = createHash('sha256');
    for await (const bytes of createReadStream(join(directory, name))) hash.update(bytes);
    const digest = hash.digest('hex');
    const sidecar = `${name}.sha256`;
    if (present.has(sidecar)) {
      if ((await lstat(join(directory, sidecar))).size > 512)
        throw new Error('Invalid checksum metadata size.');
      const text = await readFile(join(directory, sidecar), 'utf8');
      const match = text.match(/^([A-Fa-f0-9]{64})  ([^\r\n]+)\r?\n?$/);
      if (!match || match[1].toLowerCase() !== digest || match[2] !== name)
        throw new Error(`Download checksum mismatch: ${name}`);
    } else {
      if (!write) throw new Error(`Missing download checksum: ${name}`);
      pending.push([join(directory, sidecar), `${digest}  ${name}\n`]);
    }
  }
  // Validate every existing sidecar before writing any new ones. Exclusive writes preserve
  // user files and detect a second collector racing this process.
  for (const [path, text] of pending) await writeFile(path, text, { flag: 'wx' });
  return files.length;
}
async function main() {
  const options = new Map();
  for (let index = 2; index < process.argv.length; index++) {
    const name = process.argv[index];
    if (options.has(name)) throw new Error('Duplicate checksum option.');
    if (['--write', '--check', '--complete'].includes(name)) {
      options.set(name, true);
      continue;
    }
    const value = process.argv[++index];
    if (!['--dir', '--version'].includes(name) || !value || value.startsWith('--'))
      throw new Error('Invalid checksum options.');
    options.set(name, value);
  }
  if (
    !options.has('--dir') ||
    !options.has('--version') ||
    options.has('--write') === options.has('--check')
  )
    throw new Error('Expected --dir PATH --version VERSION and exactly one of --write/--check.');
  const count = await checksums({
    directory: resolve(options.get('--dir')),
    version: options.get('--version'),
    write: options.has('--write'),
    complete: options.has('--complete'),
  });
  console.log(`Verified SHA-256 for ${count} release files.`);
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    await main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
