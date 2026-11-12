import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { documentFailureEvidence } from '../native/support/generation-evidence.mjs';

// Execute the runner's actual catch block, not a second copy of its policy.
// Only lifecycle/error objects are synthetic: no app, model or browser starts.
const path = new URL('../native/real-generation.mjs', import.meta.url);
const source = await readFile(path, 'utf8');
// Both boundaries are unique top-level lines in the formatted runner. Refuse
// ambiguity or a changed boundary instead of executing a different handler.
const begin = '\n} catch (error) {\n';
const end = '\n} finally {\n';
assert.equal(source.split(begin).length, 2, 'Expected exactly one top-level catch');
assert.equal(source.split(end).length, 2, 'Expected exactly one top-level finally');
const start = source.indexOf(begin) + begin.length;
const finish = source.indexOf(end, start);
assert(finish > start, 'The catch must end before the top-level finally');
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const recordFailure = new AsyncFunction(
  'result',
  'instance',
  'child',
  'verifiedProfile',
  'error',
  'join',
  'evidence',
  source.slice(start, finish),
);

async function observe({ returned = false, owned = false, verified = false } = {}) {
  const marker = 'synthetic-unverified-window-title-8dd946';
  const result = {};
  let screenshots = 0;
  const instance = returned
    ? {
        main: {
          screenshot: async () => {
            screenshots += 1;
          },
        },
      }
    : undefined;
  const failure = await recordFailure(
    result,
    instance,
    owned ? { pid: 1234 } : undefined,
    verified,
    new Error(marker),
    join,
    'inert-evidence-directory',
  ).then(
    () => null,
    (error) => error,
  );
  assert(failure instanceof Error);
  return { result, failure, screenshots, marker };
}

test('a launch rejection before instance assignment cannot expose unverified evidence', async () => {
  const state = await observe({ owned: true });
  assert(!state.result.error.includes(state.marker), 'Launcher error exposed unverified content');
  assert.match(state.result.error, /profile not verified/i);
  assert(!state.failure.message.includes(state.marker));
  assert.equal(state.screenshots, 0);
});

test('a returned unverified instance cannot expose evidence or request a screenshot', async () => {
  const state = await observe({ returned: true });
  assert(!state.result.error.includes(state.marker));
  assert.match(state.result.error, /profile not verified/i);
  assert.equal(state.screenshots, 0);
});

test('verified fixture errors retain diagnostics and the owned screenshot attempt', async () => {
  const state = await observe({ returned: true, owned: true, verified: true });
  assert(state.result.error.includes(state.marker));
  assert(state.failure.message.includes(state.marker));
  assert.equal(state.screenshots, 1);
});

test('errors before any launch retain useful asset or preflight diagnostics', async () => {
  const state = await observe();
  assert(state.result.error.includes(state.marker));
  assert.equal(state.screenshots, 0);
});

test('failed document evidence selects only its persisted request and opens databases read-only', async () => {
  const profile = await mkdtemp(join(tmpdir(), 'prism-document-evidence-'));
  await Promise.all(['documents', 'rag'].map((name) => mkdir(join(profile, name))));
  const documentPath = join(profile, 'documents', 'documents.sqlite3');
  const ragPath = join(profile, 'rag', 'conversations.sqlite3');
  const job = { documentId: 'a'.repeat(32), requestId: 'failed-request' };
  const documents = new DatabaseSync(documentPath);
  documents.exec('CREATE TABLE documents(id TEXT, rag_conversation_id TEXT)');
  documents.prepare('INSERT INTO documents VALUES (?, ?)').run(job.documentId, 'owned-document');
  documents.close();
  const rag = new DatabaseSync(ragPath);
  rag.exec(`CREATE TABLE conversation_turns(
    conversation_id TEXT, request_id TEXT, sequence INTEGER, role TEXT, payload TEXT)`);
  const insert = rag.prepare('INSERT INTO conversation_turns VALUES (?, ?, ?, ?, ?)');
  const response = {
    requestId: 'doc_failed-request',
    role: 'assistant',
    status: 'complete',
    text: '{"title":"truncated',
    finishReason: 'length',
    usage: { completion_tokens: 2048 },
    requestedModel: 'fixture',
    reportedModel: 'fixture',
  };
  insert.run('owned-document', 'doc_failed-request', 1, 'user', '{"secret":"user question"}');
  insert.run('owned-document', 'doc_old-request', 2, 'assistant', '{"secret":"old answer"}');
  insert.run('other-document', 'doc_failed-request', 3, 'assistant', '{"secret":"other answer"}');
  insert.run('owned-document', 'doc_failed-request', 4, 'assistant', JSON.stringify(response));
  rag.close();
  const before = await Promise.all([documentPath, ragPath].map((path) => readFile(path)));
  assert.deepEqual(documentFailureEvidence(profile, job), {
    documentId: job.documentId,
    requestId: job.requestId,
    ...Object.fromEntries(
      ['status', 'text', 'finishReason', 'usage', 'requestedModel', 'reportedModel'].map((key) => [
        key,
        response[key],
      ]),
    ),
  });
  const after = await Promise.all([documentPath, ragPath].map((path) => readFile(path)));
  assert.deepEqual(after, before);
  assert.throws(() => documentFailureEvidence(profile, { ...job, requestId: "' OR 1=1 --" }));
  assert.throws(() => documentFailureEvidence(profile, { ...job, requestId: 'missing' }));
  const inconsistent = new DatabaseSync(ragPath);
  inconsistent
    .prepare('UPDATE conversation_turns SET payload = ? WHERE sequence = 4')
    .run(JSON.stringify({ ...response, requestId: 'doc_other-request' }));
  inconsistent.close();
  assert.throws(() => documentFailureEvidence(profile, job));
});
