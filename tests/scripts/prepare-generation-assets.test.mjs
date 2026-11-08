// Inert streaming/download-helper fixtures only. No network, app or model runs.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  AssetPreparationError,
  prepareGenerationAssets,
} from '../../scripts/prepare-generation-assets.mjs';

test('the CLI rejects missing arguments with a static error and no download', () => {
  const script = fileURLToPath(
    new URL('../../scripts/prepare-generation-assets.mjs', import.meta.url),
  );
  const result = spawnSync(process.execPath, [script], { encoding: 'utf8', windowsHide: true });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.deepEqual(JSON.parse(result.stderr.trim().split(/\r?\n/).at(-1)), {
    passed: false,
    code: 'arguments',
  });
  assert(!result.stderr.includes('file:///'));
});

async function fixture(t) {
  const parent = await mkdtemp(join(tmpdir(), 'prism-assets-helper-test-'));
  const owned = await realpath(parent);
  const base = await realpath(tmpdir());
  t.after(async () => {
    // Verify the resolved recursive-delete target is the exact new fixture,
    // underneath the intended temp folder, before removing it.
    assert.equal(await realpath(parent), owned);
    assert.equal(dirname(owned), base);
    assert(basename(owned).startsWith('prism-assets-helper-test-'));
    await rm(parent, { recursive: true, force: true });
  });
  const assets = Object.fromEntries(
    ['generation', 'embedding', 'runtime'].map((kind) => {
      const bytes = Buffer.from(`inert-${kind}`);
      return [
        kind,
        {
          filename: `${kind}.fixture`,
          bytes: bytes.length,
          sha256: createHash('sha256').update(bytes).digest('hex'),
          url: `https://fixture.invalid/${kind}`,
        },
      ];
    }),
  );
  const calls = [];
  const fetchImpl = async (url, options) => {
    calls.push({ url: String(url), redirect: options.redirect });
    const kind = new URL(url).pathname.slice(1);
    const bytes = Buffer.from(`inert-${kind}`);
    return new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(bytes.subarray(0, 2));
          controller.enqueue(bytes.subarray(2));
          controller.close();
        },
      }),
    );
  };
  return { parent, directory: join(parent, 'assets'), assets, fetchImpl, calls };
}

test('streams all pinned bytes and publishes only validated files in a new directory', async (t) => {
  const f = await fixture(t);
  const result = await prepareGenerationAssets(f.directory, f);
  assert(result.passed && !result.actualInference);
  assert.equal(f.calls.length, 3);
  for (const [kind, file] of Object.entries(result.files)) {
    assert.equal((await readFile(file.path)).toString(), `inert-${kind}`);
    assert.equal(file.sha256, f.assets[kind].sha256);
  }
  assert.deepEqual(
    JSON.parse(await readFile(join(f.directory, 'assets.json'), 'utf8')),
    result.files,
  );
  assert.equal((await readdir(f.directory)).filter((name) => name.includes('.partial-')).length, 0);
});

test('existing directories and traversal-valued pins are rejected before fetching or overwriting', async (t) => {
  const f = await fixture(t);
  await mkdir(f.directory);
  await writeFile(join(f.directory, 'keep.txt'), 'canary');
  await assert.rejects(
    prepareGenerationAssets(f.directory, f),
    (error) => error.code === 'directory',
  );
  assert.equal(await readFile(join(f.directory, 'keep.txt'), 'utf8'), 'canary');
  f.assets.generation.filename = '../escape';
  await assert.rejects(
    prepareGenerationAssets(join(f.parent, 'other'), f),
    (error) => error.code === 'manifest',
  );
  assert.equal(f.calls.length, 0);
  assert.deepEqual((await readdir(f.parent)).sort(), ['assets']);
});

test('wrong hashes and oversized bodies never publish a final asset', async (t) => {
  for (const mode of ['hash', 'size']) {
    const f = await fixture(t);
    if (mode === 'hash') f.assets.generation.sha256 = '0'.repeat(64);
    else f.assets.generation.bytes = 2;
    await assert.rejects(
      prepareGenerationAssets(f.directory, f),
      (error) => error instanceof AssetPreparationError && error.code === mode,
    );
    const names = await readdir(f.directory);
    assert(!names.includes('generation.fixture') && !names.includes('assets.json'));
    const status = JSON.parse(await readFile(join(f.directory, 'status.json'), 'utf8'));
    assert.equal(status.passed, false);
    assert.equal(status.error.code, mode);
  }
});

test('HTTPS redirects are followed manually and a downgrade is rejected before its request', async (t) => {
  const f = await fixture(t);
  let requests = 0;
  const fetchImpl = async (url, options) => {
    requests += 1;
    if (new URL(url).hostname === 'fixture.invalid')
      return new Response(null, {
        status: 302,
        headers: { location: `https://cdn.invalid${new URL(url).pathname}` },
      });
    return f.fetchImpl(url, options);
  };
  assert((await prepareGenerationAssets(f.directory, { ...f, fetchImpl })).passed);
  assert.equal(requests, 6);
  const next = await fixture(t);
  let downgradeRequests = 0;
  await assert.rejects(
    prepareGenerationAssets(next.directory, {
      ...next,
      fetchImpl: async () => {
        downgradeRequests += 1;
        return new Response(null, {
          status: 302,
          headers: { location: 'http://fixture.invalid/insecure' },
        });
      },
    }),
    (error) => error.code === 'redirect',
  );
  assert.equal(downgradeRequests, 1);
});

test('upstream exceptions containing signed URLs are reduced to static status codes', async (t) => {
  const f = await fixture(t);
  const sensitive = 'https://cdn.invalid/file?secret-fixture-token=do-not-log';
  await assert.rejects(
    prepareGenerationAssets(f.directory, {
      ...f,
      fetchImpl: async () => {
        throw new Error(sensitive);
      },
    }),
    (error) => error.code === 'transfer' && !String(error).includes(sensitive),
  );
  const status = await readFile(join(f.directory, 'status.json'), 'utf8');
  assert(!status.includes(sensitive) && !status.includes('do-not-log'));
  assert.equal(JSON.parse(status).error.code, 'transfer');
});

test('deadline failure is recorded without publishing or retrying the asset', async (t) => {
  const f = await fixture(t);
  let calls = 0;
  await assert.rejects(
    prepareGenerationAssets(f.directory, {
      ...f,
      timeoutMs: 10,
      fetchImpl: (_url, { signal }) =>
        new Promise((_done, reject) => {
          calls += 1;
          const timer = setTimeout(() => reject(new Error('fixture deadline missed')), 500);
          signal.addEventListener(
            'abort',
            () => {
              clearTimeout(timer);
              reject(signal.reason);
            },
            { once: true },
          );
        }),
    }),
    (error) => error.code === 'timeout',
  );
  assert.equal(calls, 1);
  assert(!(await readdir(f.directory)).includes('assets.json'));
});
