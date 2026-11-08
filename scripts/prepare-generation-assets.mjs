// Data preparation for the opt-in native gate. Never executes an asset/model.
import { createHash, randomUUID } from 'node:crypto';
import { createWriteStream } from 'node:fs';
import { link, lstat, mkdir, readFile, unlink, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { pathToFileURL } from 'node:url';

const kinds = ['generation', 'embedding', 'runtime'];
const pinsPath = new URL('../tests/fixtures/generation/pins.json', import.meta.url);

export class AssetPreparationError extends Error {
  constructor(code, kind) {
    // Fetch/redirect/body errors can contain signed CDN URLs. Do not keep their
    // message, body or cause in the status file or CLI error output.
    super(`Native generation asset preparation failed (${code})`);
    this.code = code;
    this.kind = kind;
  }
}
function validate(asset) {
  try {
    return (
      /^[A-Za-z0-9][A-Za-z0-9._-]{0,119}$/.test(asset.filename) &&
      !['qa-owner.txt', 'status.json', 'assets.json'].includes(asset.filename) &&
      Number.isSafeInteger(asset.bytes) &&
      asset.bytes > 0 &&
      /^[0-9a-f]{64}$/.test(asset.sha256) &&
      new URL(asset.url).protocol === 'https:'
    );
  } catch {
    return false;
  }
}

async function download(asset, directory, kind, fetchImpl, timeoutMs) {
  const signal = AbortSignal.timeout(timeoutMs);
  const fail = (code) => new AssetPreparationError(code, kind);
  try {
    let url = new URL(asset.url);
    let response;
    for (let redirects = 0; ; redirects += 1) {
      response = await fetchImpl(url, { redirect: 'manual', signal });
      if (![301, 302, 303, 307, 308].includes(response.status)) break;
      const location = response.headers.get('location');
      await response.body?.cancel();
      if (!location || redirects >= 5) throw fail('redirect');
      url = new URL(location, url);
      if (url.protocol !== 'https:') throw fail('redirect');
    }
    if (!response.ok) throw fail('http');
    if (!response.body) throw fail('body');
    const partial = join(directory, `${asset.filename}.partial-${randomUUID()}`);
    const target = join(directory, asset.filename);
    const hash = createHash('sha256');
    let bytes = 0;
    const checked = new Transform({
      transform(chunk, _encoding, done) {
        bytes += chunk.length;
        if (bytes > asset.bytes) return done(fail('size'));
        hash.update(chunk);
        done(null, chunk);
      },
    });
    await pipeline(
      Readable.fromWeb(response.body),
      checked,
      createWriteStream(partial, { flags: 'wx' }),
      { signal },
    );
    if (bytes !== asset.bytes) throw fail('size');
    if (hash.digest('hex') !== asset.sha256) throw fail('hash');
    // Both paths are in our newly created directory on one volume. Publishing
    // by hard link refuses an existing destination instead of overwriting it.
    await link(partial, target);
    await unlink(partial);
    return { path: target, bytes, sha256: asset.sha256 };
  } catch (error) {
    if (error instanceof AssetPreparationError) throw error;
    throw fail(signal.aborted ? 'timeout' : 'transfer');
  }
}

/** The CLI always uses the repository pins. Injection is for inert helper tests. */
export async function prepareGenerationAssets(
  output,
  { assets, fetchImpl = fetch, timeoutMs = 300_000 } = {},
) {
  assets ??= JSON.parse(await readFile(pinsPath, 'utf8'));
  if (
    !Number.isInteger(timeoutMs) ||
    timeoutMs <= 0 ||
    timeoutMs > 2 ** 32 - 1 ||
    !kinds.every((kind) => validate(assets[kind])) ||
    new Set(kinds.map((kind) => assets[kind].filename)).size !== kinds.length
  )
    throw new AssetPreparationError('manifest');
  const directory = resolve(output);
  try {
    // An existing folder may contain user assets or a failed earlier attempt.
    // Each invocation gets a new directory; neither case is overwritten.
    await mkdir(directory);
    const metadata = await lstat(directory);
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error();
  } catch {
    throw new AssetPreparationError('directory');
  }
  await writeFile(join(directory, 'qa-owner.txt'), 'Prism immutable generation assets v1\n', {
    flag: 'wx',
  });
  const status = { passed: false, actualInference: false, files: {} };
  try {
    for (const kind of kinds) {
      status.current = kind;
      status.files[kind] = await download(assets[kind], directory, kind, fetchImpl, timeoutMs);
      await writeFile(join(directory, 'status.json'), JSON.stringify(status, null, 2));
    }
    status.current = null;
    status.passed = true;
    await writeFile(join(directory, 'assets.json'), JSON.stringify(status.files, null, 2), {
      flag: 'wx',
    });
    return status;
  } catch (error) {
    const safe =
      error instanceof AssetPreparationError
        ? error
        : new AssetPreparationError('storage', status.current);
    status.passed = false;
    status.error = { code: safe.code, kind: safe.kind };
    throw safe;
  } finally {
    await writeFile(join(directory, 'status.json'), JSON.stringify(status, null, 2));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    if (process.argv.length !== 3) throw new AssetPreparationError('arguments');
    const result = await prepareGenerationAssets(process.argv[2]);
    console.log(JSON.stringify(result));
  } catch (error) {
    const safe =
      error instanceof AssetPreparationError ? error : new AssetPreparationError('storage');
    console.error(JSON.stringify({ passed: false, code: safe.code, kind: safe.kind }));
    process.exitCode = 1;
  }
}
