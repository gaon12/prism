// Actual Windows 8.3 aliases, real content/RAG/settings and isolated native app.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile, rename, writeFile } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

if (process.platform !== 'win32') {
  console.log(JSON.stringify({ skipped: true, reason: 'Windows 8.3 path integration fixture' }));
  process.exit(0);
}
const directory = await mkdtemp(join(tmpdir(), 'prism-short-native-'));
const docs = join(directory, 'Long authorized content folder');
const offline = join(directory, 'Long removable document folder');
const metadata = join(directory, 'Filename only');
const profile = join(docs, 'Long application profile');
await Promise.all(
  [docs, offline, metadata, join(profile, 'embedding')].map((path) =>
    mkdir(path, { recursive: true }),
  ),
);
await writeFile(join(docs, 'source.md'), 'publicneedle PUBLIC_EVIDENCE_SHORT_PATH');
await writeFile(join(offline, 'source.md'), 'offlineneedle REMOVABLE_EVIDENCE');
await writeFile(join(profile, 'embedding', 'secret.md'), 'publicneedle privatecachecanary');
await writeFile(join(metadata, 'secret.md'), 'publicneedle metadatacanary');
const script = `
$ErrorActionPreference='Stop'
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class PrismShortPaths {
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
  public static extern uint GetShortPathName(string path,StringBuilder result,uint capacity);
}
'@
$prismPaths=ConvertFrom-Json $env:PRISM_SHORT_NATIVE_PATHS
$prismShort=@()
foreach($prismPath in $prismPaths) {
  $prismBuffer=New-Object System.Text.StringBuilder 32768
  $prismLength=[PrismShortPaths]::GetShortPathName($prismPath,$prismBuffer,32768)
  if($prismLength -eq 0 -or $prismLength -ge 32768) { throw 'Cannot obtain fixture short path' }
  $prismShort += $prismBuffer.ToString()
}
ConvertTo-Json -InputObject $prismShort -Compress
`;
const longPaths = [docs, offline, profile];
const short = JSON.parse(
  execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
    env: { ...process.env, PRISM_SHORT_NATIVE_PATHS: JSON.stringify(longPaths) },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30000,
  }),
);
const key = (path) =>
  path
    .replaceAll('\\', '/')
    .replace(/^\/\/\?\//, '')
    .toLowerCase();
if (!short.some((path, index) => key(path) !== key(longPaths[index]))) {
  if (process.env.PRISM_REQUIRE_SHORT_PATH === '1')
    throw new Error('Fixture volume must supply actual short aliases');
  console.log(
    JSON.stringify({ skipped: true, directory, reason: 'Volume does not create 8.3 aliases' }),
  );
  process.exit(0);
}
assert(short.some((path) => path.includes('~')));
const calls = [],
  sockets = new Set();
const server = createServer((request, response) => {
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', () => {
    calls.push(JSON.parse(Buffer.concat(chunks).toString()));
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end(
      JSON.stringify({
        choices: [{ message: { content: '합성 근거 응답 [S1]' }, finish_reason: 'stop' }],
      }),
    );
  });
});
server.on('connection', (socket) => {
  sockets.add(socket);
  socket.on('close', () => sockets.delete(socket));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F22',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    generation: {
      local: {
        endpoint: `http://127.0.0.1:${server.address().port}/v1`,
        model: 'synthetic',
        maxTokens: 256,
        tokenParameter: 'maxTokens',
        temperature: null,
      },
    },
    scopes: [
      {
        id: 'docs',
        name: '자료',
        path: short[0],
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
      {
        id: 'offline',
        name: '이동식 자료',
        path: short[1],
        filenameEnabled: true,
        contentEnabled: true,
        externalAllowed: false,
      },
      {
        id: 'metadata',
        name: '파일명 전용',
        path: metadata,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
    ],
  }),
);
let instance;
try {
  instance = await launch(short[2]);
  const { invoke, main } = instance;
  const text = (query) =>
    invoke('search_content', {
      query,
      extension: '',
      scopeId: null,
      requestId: crypto.randomUUID(),
    });
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'short alias scopes exclude app assets and filename-only content');
  assert.equal((await invoke('runtime_info')).shortcutError, null);
  assert.equal((await invoke('model_status')).modelReady, false);
  assert.equal((await text('publicneedle')).hits.length, 1);
  assert.equal((await text('privatecachecanary')).hits.length, 0);
  assert.equal((await text('metadatacanary')).hits.length, 0);
  const created = await invoke('rag_create', { title: 'Short path fixture' });
  const answer = await invoke('rag_ask', {
    request: {
      conversationId: created.id,
      requestId: crypto.randomUUID(),
      question: 'publicneedle',
      scopeIds: ['docs'],
      mode: 'documents',
      sourcesHidden: false,
    },
  });
  assert.equal(answer.assistantTurn.status, 'complete');
  assert.equal(calls.length, 1);
  assert(JSON.stringify(calls[0]).includes('PUBLIC_EVIDENCE_SHORT_PATH'));
  assert(!JSON.stringify(calls[0]).includes('privatecachecanary'));
  assert(!JSON.stringify(calls[0]).includes('metadatacanary'));
  assert(!JSON.stringify(calls[0]).includes(directory));
  const settings = await invoke('get_settings');
  const saved = await invoke('save_settings', { settings, expectedRevision: settings.revision });
  for (const [id, path] of [
    ['docs', docs],
    ['offline', offline],
  ]) {
    const scope = saved.scopes.find((scope) => scope.id === id);
    assert.equal(key(scope.path), key(realpathSync(path)));
    assert.equal(scope.externalAllowed, false);
    assert.equal(scope.contentEnabled, true);
  }
  const persisted = JSON.parse(await readFile(join(profile, 'settings.json'), 'utf8'));
  assert.equal(
    key(persisted.scopes.find((scope) => scope.id === 'offline').path),
    key(realpathSync(offline)),
  );
  const moved = join(directory, 'Temporarily disconnected scope');
  for (const path of [offline, moved])
    assert(resolve(path).startsWith(resolve(directory) + sep), 'rename stays inside owned fixture');
  await rename(offline, moved);
  await invoke('content_refresh');
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.issues.some((issue) => issue.code === 'io');
  }, 'unavailable saved long scope reported');
  assert.equal((await text('offlineneedle')).hits.length, 0);
  assert.equal((await text('privatecachecanary')).hits.length, 0);
  await rename(moved, offline);
  await invoke('content_refresh');
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 2;
  }, 'restored saved scope');
  assert.equal((await text('offlineneedle')).hits.length, 1);
  assert.equal(calls.length, 1);
  assert.deepEqual(instance.errors, []);
  assert.equal(await main.title(), 'Prism');
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      actualShortPaths: short,
      contentFiles: 2,
      privateCacheHits: 0,
      metadataContentHits: 0,
      externalCalls: 0,
    }),
  );
} catch (error) {
  console.error(JSON.stringify({ directory, actualShortPaths: short }));
  throw error;
} finally {
  if (instance) await shutdown(instance);
  for (const socket of sockets) socket.destroy();
  await new Promise((done) => server.close(done));
}
