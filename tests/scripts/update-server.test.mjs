// Runs the real server/update/index.php with the PHP command-line server and checks what Prism
// would receive. Skipped when no `php` command is installed.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { after, before, test } from 'node:test';

const php = process.env.PHP_BINARY ?? 'php';
const hasPhp = spawnSync(php, ['-v'], { windowsHide: true }).status === 0;
const script = resolve('server/update/index.php');

// A complete, valid data file; each test changes one thing.
const valid = () => ({
  version: '0.2.0',
  notes: 'Prism 0.2.0',
  pub_date: '2026-11-17T00:00:00.000Z',
  platforms: {
    'windows-x86_64': {
      signature: 'dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Q=',
      url: 'https://updates.example.com/prism/Prism_0.2.0_x64-setup.exe',
      sha256: 'a'.repeat(64),
    },
  },
});

let folder;
let server;
let base;
before(async () => {
  if (!hasPhp) return;
  folder = mkdtempSync(join(tmpdir(), 'prism-update-server-'));
  copyFileSync(script, join(folder, 'index.php'));
  const probe = createServer();
  await new Promise((done) => probe.listen(0, '127.0.0.1', done));
  const { port } = probe.address();
  await new Promise((done) => probe.close(done));
  server = spawn(php, ['-S', `127.0.0.1:${port}`, '-t', folder], {
    windowsHide: true,
    stdio: 'ignore',
  });
  base = `http://127.0.0.1:${port}/index.php`;
  // Wait until the PHP server answers.
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      await fetch(base);
      return;
    } catch {
      await new Promise((done) => setTimeout(done, 100));
    }
  }
  throw new Error('PHP test server did not start.');
});
after(() => {
  server?.kill();
  if (folder) rmSync(folder, { recursive: true, force: true });
});

/** Writes `latest.json` (an object, or raw text) and asks the script for it. */
async function serve(data, init) {
  writeFileSync(
    join(folder, 'latest.json'),
    typeof data === 'string' ? data : JSON.stringify(data),
  );
  const response = await fetch(base, init);
  return { status: response.status, headers: response.headers, body: await response.json() };
}

test('a valid data file is returned with only the checked fields', { skip: !hasPhp }, async () => {
  const data = valid();
  data.extra = 'ignored';
  data.platforms['windows-x86_64'].extra = 'ignored';
  const { status, headers, body } = await serve(data);
  assert.equal(status, 200);
  assert.match(headers.get('content-type'), /^application\/json/);
  assert.equal(headers.get('cache-control'), 'no-store');
  delete data.extra;
  delete data.platforms['windows-x86_64'].extra;
  assert.deepEqual(body, data);
});

test('broken data files are refused with a generic 503', { skip: !hasPhp }, async () => {
  const broken = [
    'not json',
    { ...valid(), version: 'v0.2' },
    { ...valid(), pub_date: 'someday' },
    { ...valid(), platforms: {} },
    { ...valid(), platforms: [valid().platforms['windows-x86_64']] },
  ];
  const entry = (change) => {
    const data = valid();
    Object.assign(data.platforms['windows-x86_64'], change);
    return data;
  };
  broken.push(
    entry({ url: 'http://updates.example.com/a.exe' }),
    entry({ url: 'https://user:pass@updates.example.com/a.exe' }),
    entry({ url: 'https://updates.example.com/a.exe#x' }),
    entry({ sha256: 'A'.repeat(64) }),
    entry({ sha256: 'a'.repeat(63) }),
    entry({ signature: ' ' }),
  );
  for (const data of broken) {
    const { status, body } = await serve(data);
    assert.equal(status, 503, JSON.stringify(data));
    // No file paths or PHP details reach the client.
    assert.deepEqual(body, { error: 'Update information is temporarily unavailable.' });
  }
});

test('oversized data files and writes are refused', { skip: !hasPhp }, async () => {
  const large = valid();
  large.notes = 'x'.repeat(70000);
  assert.equal((await serve(large)).status, 503);
  const posted = await serve(valid(), { method: 'POST' });
  assert.equal(posted.status, 405);
  assert.equal(posted.headers.get('allow'), 'GET, HEAD');
});
