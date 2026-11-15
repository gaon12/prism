// Opt-in real Kaikki HTTPS transfer/cancellation; no replacement HTTP responses.
// Only an owned portable profile and generated CC0 seed pack are modified.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, readdir, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(
  process.env.PRISM_TEST_DICTIONARY_NETWORK,
  '1',
  'Explicit real-network opt-in required',
);
assert.equal(process.platform, 'win32');
assert(process.env.PRISM_TEST_EXECUTABLE, 'Supply a source-qualified executable');
const expectedRelease = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(expectedRelease), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1');
const expectedSha = process.env.PRISM_TEST_APP_SHA256;
assert.match(expectedSha ?? '', /^[a-f0-9]{64}$/);
const originalExecutable = resolve(process.env.PRISM_TEST_EXECUTABLE);
async function hash(path) {
  const digest = createHash('sha256');
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return digest.digest('hex');
}
assert.equal(await hash(originalExecutable), expectedSha);
const root = await mkdtemp(join(tmpdir(), 'prism-native-dictionary-download-'));
const application = join(root, 'app');
const data = join(application, 'PrismData');
const dictionary = join(data, 'dictionary');
const evidence = join(root, 'evidence');
await mkdir(dictionary, { recursive: true });
await mkdir(evidence);
const executable = join(application, 'prism.exe');
await copyFile(originalExecutable, executable);
await writeFile(join(application, 'prism.portable'), '');
assert.equal(await hash(executable), expectedSha);
await writeFile(
  join(data, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    language: 'ko',
    theme: 'light',
    startOnLogin: false,
    rememberSearch: false,
    shortcut: 'Ctrl+Alt+Shift+F10',
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    automaticSemantic: false,
    generation: { local: null, external: null },
    scopes: [],
  }),
);
const protectedNames = ['pack-ko.download', '.prism-dictionary-preserved-stage'];
for (const name of protectedNames)
  await writeFile(join(dictionary, name), `Generated staging-preservation canary: ${name}`);
const protectedHashes = Object.fromEntries(
  await Promise.all(protectedNames.map(async (name) => [name, await hash(join(dictionary, name))])),
);
const manifest = {
  schemaVersion: 1,
  language: 'ko',
  name: 'Generated CC0 cancellation seed',
  version: '1',
  sourceUrl: 'https://example.org/generated-dictionary-fixture',
  license: 'CC0',
  licenseUrl: 'https://creativecommons.org/publicdomain/zero/1.0/',
  definitionLanguage: 'en',
};
const seed = join(root, 'seed.jsonl');
const definition = 'Generated seed definition preserved after real transfer cancellation';
await writeFile(
  seed,
  `${JSON.stringify({ prismDictionary: manifest })}\n${JSON.stringify({ term: '사과', definitions: [definition] })}\n`,
);
const seedSha = await hash(seed);
const checks = [];
let instance, operation, failure, lastJob, settled;
try {
  instance = await launch(data, {
    executable,
    portable: true,
    productionPortable: expectedRelease === '1',
    cdpPort: process.env.PRISM_APP_CDP_PORT ? Number(process.env.PRISM_APP_CDP_PORT) : undefined,
  });
  const runtime = await instance.invoke('runtime_info');
  assert.equal(runtime.native, true);
  assert.equal(runtime.development, expectedRelease !== '1');
  assert.equal(runtime.installKind, 'portable');
  const settings = await instance.invoke('get_settings');
  assert.equal(settings.clipboard.enabled, false);
  assert.deepEqual(settings.scopes, []);
  assert.equal(settings.generation.local, null);
  assert.equal(settings.generation.external, null);
  assert.deepEqual(await instance.invoke('dictionary_packs'), []);
  const imported = await instance.invoke('dictionary_import', { path: seed, manifest: null });
  assert.equal(imported.entries, 1);
  const packsBefore = await instance.invoke('dictionary_packs');
  assert.equal(packsBefore.length, 1);
  const before = await instance.invoke('dictionary_lookup', {
    language: 'ko',
    word: '사과',
    offline: true,
  });
  assert.equal(before.source, 'offline');
  assert.equal(before.definitions[0].text, definition);
  checks.push({ name: 'owned portable startup and generated offline seed', runtime });

  operation = instance.invoke('dictionary_download', { language: 'ko' }).then(
    (value) => (settled = { ok: true, value }),
    (error) => (settled = { ok: false, error }),
  );
  await until(
    async () => {
      lastJob = await instance.invoke('dictionary_job');
      assert(!settled, `Transfer ended before observing actual bytes: ${JSON.stringify(settled)}`);
      if (lastJob.phase === 'downloading') return lastJob.bytes > 0;
      assert(
        ['idle', 'ready'].includes(lastJob.phase),
        `Unexpected phase: ${JSON.stringify(lastJob)}`,
      );
      return false;
    },
    'real official Korean dictionary HTTPS bytes',
    30_000,
  );
  assert.equal(lastJob.language, 'ko');
  const liveStageNames = (await readdir(dictionary)).filter(
    (name) => name.startsWith('.prism-dictionary-') && !protectedNames.includes(name),
  );
  assert.equal(liveStageNames.length, 1);
  const stageBytes = (await stat(join(dictionary, liveStageNames[0]))).size;
  assert(stageBytes > 0, 'Actual streamed bytes must be in a distinct owned stage');
  const observedJob = lastJob;
  const cancelStarted = Date.now();
  await instance.invoke('dictionary_cancel');
  await until(() => settled !== undefined, 'real dictionary cancellation acknowledgement', 10_000);
  await operation;
  assert.equal(settled.ok, false);
  assert.equal(settled.error.code, 'cancelled');
  lastJob = await instance.invoke('dictionary_job');
  assert.equal(lastJob.phase, 'cancelled');
  assert.equal(lastJob.error.code, 'cancelled');
  const cancelMs = Date.now() - cancelStarted;
  assert.deepEqual(await instance.invoke('dictionary_packs'), packsBefore);
  const after = await instance.invoke('dictionary_lookup', {
    language: 'ko',
    word: '사과',
    offline: true,
  });
  assert.deepEqual(after, before);
  for (const [name, digest] of Object.entries(protectedHashes))
    assert.equal(await hash(join(dictionary, name)), digest);
  assert.deepEqual(
    (await readdir(dictionary)).filter((name) => name.startsWith('.prism-dictionary-')).sort(),
    ['.prism-dictionary-preserved-stage'],
  );
  assert.equal(await hash(seed), seedSha);
  assert.equal(await hash(executable), expectedSha);
  assert.equal(await hash(originalExecutable), expectedSha);
  assert.deepEqual(instance.errors, []);
  checks.push({
    name: 'real HTTPS stream cancelled; generated pack and pre-existing stages preserved',
    endpoint: 'https://kaikki.org/dictionary/Korean/kaikki.org-dictionary-Korean.jsonl',
    responsesReplaced: false,
    observedJob,
    observedStageBytes: stageBytes,
    terminalJob: lastJob,
    cancelMs,
  });
} catch (error) {
  failure = { message: String(error), stack: error.stack, lastJob, settled };
  await instance?.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
} finally {
  try {
    if (instance) {
      if (operation && !settled) {
        await instance.invoke('dictionary_cancel');
        await until(() => settled !== undefined, 'cleanup download cancellation', 10_000);
      }
      await shutdown(instance);
    }
  } catch (error) {
    failure ??= { message: `Owned app cleanup failed: ${error}` };
  }
  const result = {
    passed: !failure,
    root,
    executable,
    executableSha256: expectedSha,
    expectedRelease,
    checks,
    failure,
    protectedHashes,
    seedSha,
    limits: [
      'Opt-in official HTTPS transfer/cancellation only, not full corpus download/import or definition quality.',
      'Existing pack is a generated CC0 seed; no user profile, provider, model, installed service or OS clipboard is used.',
      'Windows portable ordinary-filesystem scope; no other-platform, low-memory performance or symlink claim.',
    ],
  };
  await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
}
if (failure) process.exitCode = 1;
