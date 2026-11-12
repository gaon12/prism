// Opt-in native test: actual pinned models, real SQLite/Lance and saved revisions.
// Browser plugin not available: inspect the bundled Windows WebView through CDP.
// No generated response, source search result or model status is substituted.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream, closeSync, openSync } from 'node:fs';
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  stat,
  writeFile,
} from 'node:fs/promises';
import { createServer } from 'node:net';
import { freemem, tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launch, shutdown, until } from './harness.mjs';
import { documentFailureEvidence } from './support/generation-evidence.mjs';
import { inspectHwpx } from './hwpx-inspect.mjs';
import { hasTableHeaders } from './support/table-headers.mjs';

assert.equal(process.platform, 'win32', 'This actual WebView2/model gate requires Windows');
assert.equal(process.arch, 'x64', 'Use the pinned Windows x64 CPU runtime');
assert(
  process.argv.slice(2).every((arg) => arg === '--verify-assets'),
  'Unknown argument',
);
const verifyOnly = process.argv.includes('--verify-assets');
const fixtures = fileURLToPath(new URL('../fixtures/generation/', import.meta.url));
const pins = JSON.parse(await readFile(join(fixtures, 'pins.json'), 'utf8'));
const inputs = Object.fromEntries(
  ['generation', 'embedding', 'runtime'].map((kind) => {
    const variable = `PRISM_REAL_${kind.toUpperCase()}`;
    assert(process.env[variable], `Set ${variable} to the pinned asset; this test never downloads`);
    return [kind, resolve(process.env[variable])];
  }),
);
const directory = await mkdtemp(join(tmpdir(), 'prism-real-generation-'));
const evidence = join(directory, 'evidence');
await mkdir(evidence);
await writeFile(join(directory, 'qa-owner.txt'), 'Prism fresh actual generation gate v1\n');
const result = {
  passed: false,
  verificationOnly: verifyOnly,
  responsesReplaced: false,
  actualInference: false,
  evidence,
  assetPins: pins,
};
if (process.env.PRISM_TEST_SOURCE_SHA) {
  assert(/^[0-9a-f]{40}$/i.test(process.env.PRISM_TEST_SOURCE_SHA), 'Invalid source commit');
  result.sourceCommit = process.env.PRISM_TEST_SOURCE_SHA.toLowerCase();
}
let stage = 'asset verification';
let instance, child, server, serverLog, guard;
let verifiedProfile = false;
let resourceStop = false;
let minimumAvailable = freemem();
let memorySamples = 0;
let interrupted = false;
let spawnError;
let serverFiles;
const originals = [];
const warnings = [];
const alive = (process) => process && process.exitCode === null && process.signalCode === null;
const stopOwned = () => {
  if (alive(child)) child.kill();
  if (alive(server)) server.kill();
};
const interrupt = () => {
  interrupted = true;
  stopOwned();
};
process.on('SIGINT', interrupt);
process.on('SIGTERM', interrupt);
function check() {
  assert(!interrupted, 'Actual generation gate interrupted');
  assert(!resourceStop, 'Owned processes stopped below1GiB available memory');
  if (spawnError) throw spawnError;
  if (child) assert(alive(child), 'Owned Prism process exited');
  if (server) assert(alive(server), 'Owned generation server exited');
}
async function sha(path, monitor = true) {
  const hash = createHash('sha256');
  for await (const part of createReadStream(path)) {
    if (monitor) check();
    hash.update(part);
  }
  return hash.digest('hex');
}
async function inspect(path, pin, monitor = true) {
  const metadata = await stat(path);
  assert(metadata.isFile(), 'Asset must be a regular file');
  if (pin) assert.equal(metadata.size, pin.bytes, `${pin.filename}: size mismatch`);
  const digest = await sha(path, monitor);
  if (pin) assert.equal(digest, pin.sha256, `${pin.filename}: SHA-256 mismatch`);
  return { path, bytes: metadata.size, sha256: digest };
}
const pathKey = (path) => resolve(path).replaceAll('\\', '/').toLowerCase();
function assertFacts(text) {
  const compact = text.replace(/[\s,*_]/g, '');
  assert(/(?<!\d)347000(?!\d)/.test(compact), 'Original budget must remain347000');
  assert(/(?<!\d)7명/.test(compact), 'Original attendance must remain7 people');
  assert(compact.includes('부산해운대'), 'Use the workshop location, not the distractor');
  assert(
    /2026(?:년12월0?5일|[-/.]12[-/.]0?5)/.test(compact),
    'Preserve the complete workshop date',
  );
  assert(
    /예약[^\n]{0,80}(없|미정|미기재|기재되어있지|정해지지)/.test(compact),
    'Explicitly state that the reservation number is absent from the supplied material',
  );
  assert(!compact.includes('190000'), 'The distractor budget must not enter the answer');
}
// Snapshot every installed runtime file before execution. The app has already
// authenticated the whole archive and each extracted file during model_import.
async function runtimeInventory(root) {
  const files = [];
  const walk = async (folder) => {
    for (const name of (await readdir(folder)).sort()) {
      const path = join(folder, name);
      const metadata = await lstat(path);
      assert(!metadata.isSymbolicLink(), 'No runtime links may be executed');
      if (metadata.isDirectory()) await walk(path);
      else {
        assert(metadata.isFile() && files.length < 128, 'Unexpected runtime inventory');
        files.push(await inspect(path));
      }
    }
  };
  await walk(root);
  return files;
}

try {
  const frozen = join(evidence, 'inputs');
  await mkdir(frozen);
  for (const path of [
    fileURLToPath(import.meta.url),
    fileURLToPath(new URL('./harness.mjs', import.meta.url)),
    fileURLToPath(new URL('./support/generation-evidence.mjs', import.meta.url)),
    fileURLToPath(new URL('./hwpx-inspect.mjs', import.meta.url)),
    fileURLToPath(new URL('./support/table-headers.mjs', import.meta.url)),
    ...['pins.json', '은하 워크숍.md', '별빛 세미나.md', '독서 동호회.md'].map((name) =>
      join(fixtures, name),
    ),
  ]) {
    const original = await inspect(path);
    originals.push(original);
    const snapshot = join(frozen, basename(path));
    await copyFile(path, snapshot);
    originals.push(await inspect(snapshot, { ...original, filename: basename(path) }));
  }
  if (!verifyOnly) {
    result.availableBefore = freemem();
    assert(
      result.availableBefore >= 4 * 1024 ** 3,
      'Need4GiB available before actual model inference',
    );
    minimumAvailable = result.availableBefore;
    memorySamples = 1;
    guard = setInterval(() => {
      memorySamples += 1;
      minimumAvailable = Math.min(minimumAvailable, freemem());
      if (minimumAvailable < 1024 ** 3 && !resourceStop) {
        resourceStop = true;
        stopOwned();
      }
    }, 250);
  }
  for (const [kind, path] of Object.entries(inputs))
    originals.push(await inspect(path, pins[kind]));
  result.verifiedAssets = originals.slice(-3);
  if (!verifyOnly) {
    assert.equal(
      process.env.PRISM_TEST_EXPECT_RELEASE,
      '1',
      'Use an authenticated ordinary release',
    );
    assert(process.env.PRISM_TEST_EXECUTABLE, 'Set PRISM_TEST_EXECUTABLE');
    assert(
      /^[0-9a-f]{64}$/i.test(process.env.PRISM_TEST_APP_SHA256 ?? ''),
      'Set the independently verified app SHA-256',
    );
    const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE);
    assert.equal(basename(candidate).toLowerCase(), 'prism.exe');
    const candidateInfo = await inspect(candidate);
    assert.equal(candidateInfo.sha256, process.env.PRISM_TEST_APP_SHA256.toLowerCase());
    originals.push(candidateInfo);
    result.appSha256 = candidateInfo.sha256;
    const application = join(directory, 'app');
    const profile = join(application, 'PrismData');
    const sources = join(directory, 'sources');
    await Promise.all([profile, sources].map((path) => mkdir(path, { recursive: true })));
    const executable = join(application, 'prism.exe');
    await copyFile(candidate, executable);
    await writeFile(join(application, 'prism.portable'), '');
    originals.push(await inspect(executable, { ...candidateInfo, filename: 'copied prism.exe' }));
    for (const name of ['은하 워크숍.md', '별빛 세미나.md', '독서 동호회.md']) {
      const path = join(sources, name);
      await copyFile(join(fixtures, name), path);
      originals.push(await inspect(path));
    }
    // Reserve a fresh local endpoint. No caller-controlled or remote API is used.
    const reservation = createServer();
    await new Promise((done, reject) => {
      reservation.once('error', reject);
      reservation.listen(0, '127.0.0.1', done);
    });
    const port = reservation.address().port;
    await new Promise((done) => reservation.close(done));
    const scopeId = `actual_generation_${randomUUID()}`;
    await writeFile(
      join(profile, 'settings.json'),
      JSON.stringify({
        onboardingVersion: 1,
        language: 'ko',
        theme: 'light',
        startOnLogin: false,
        shortcut: 'Ctrl+Alt+Shift+F9',
        updates: { autoCheck: false },
        clipboard: { enabled: false, onboarded: true },
        ocr: { enabled: false },
        generation: {
          mode: 'local',
          fallback: false,
          allowExternalQuestions: false,
          localWithoutApiKey: true,
          localStructuredDocuments: false,
          retrieval: 'hybrid',
          topK: null,
          maxContextChars: null,
          local: {
            endpoint: `http://127.0.0.1:${port}/v1`,
            model: 'qwen3-local-qa',
            maxTokens: 2048,
            tokenParameter: 'maxTokens',
            temperature: 0,
          },
        },
        scopes: [
          {
            id: scopeId,
            name: '시험용 프로젝트 문서',
            path: sources,
            filenameEnabled: true,
            contentEnabled: true,
            externalAllowed: false,
          },
        ],
      }),
    );
    stage = 'fresh production portable profile';
    const launchFixture = async () => {
      verifiedProfile = false;
      instance = await launch(profile, {
        executable,
        portable: true,
        productionPortable: true,
        cdpPort: process.env.PRISM_APP_CDP_PORT
          ? Number(process.env.PRISM_APP_CDP_PORT)
          : undefined,
        onSpawn: (owned) => {
          child = owned;
        },
      });
      const settings = await instance.invoke('get_settings');
      assert.equal(settings.scopes.length, 1);
      assert.equal(settings.scopes[0].id, scopeId, 'CDP must reach the fresh fixture profile');
      assert.equal(pathKey(settings.scopes[0].path), pathKey(sources));
      verifiedProfile = true;
      const runtime = await instance.invoke('runtime_info');
      assert.equal(runtime.installKind, 'portable');
      assert.equal(runtime.development, false);
      assert.equal(runtime.startupError, null);
      assert(
        !settings.clipboard.enabled &&
          !settings.ocr.enabled &&
          !settings.startOnLogin &&
          !settings.updates.autoCheck,
      );
      assert(
        !settings.scopes[0].externalAllowed &&
          !settings.generation.allowExternalQuestions &&
          !settings.generation.fallback,
      );
      assert.equal(settings.generation.local.endpoint, `http://127.0.0.1:${port}/v1`);
      assert.equal(settings.generation.mode, 'local');
      assert.equal(settings.generation.localWithoutApiKey, true);
      assert.equal(settings.generation.retrieval, 'hybrid');
      assert.equal(settings.generation.external, null);
      assert.equal(settings.generation.local.model, 'qwen3-local-qa');
      assert.equal(settings.generation.local.maxTokens, 2048);
      assert.equal(settings.generation.local.temperature, 0);
      assert.equal(settings.generation.topK, null);
      assert.equal(settings.generation.maxContextChars, null);
      for (const page of [instance.main, instance.quick])
        page.on('console', (entry) => {
          if (entry.type() === 'warning') warnings.push(entry.text());
        });
      result.runtime = runtime;
      return settings;
    };
    const initial = await launchFixture();
    assert.equal(initial.generation.localStructuredDocuments, false);
    assert.equal(
      (await instance.invoke('rag_list', { query: '', limit: 100, project: null })).length,
      0,
    );
    assert.equal((await instance.invoke('doc_list', { query: '', limit: 100 })).length, 0);
    stage = 'actual embedding and runtime import';
    await instance.invoke('model_import', { path: inputs.embedding, runtime: false });
    await instance.invoke('model_import', { path: inputs.runtime, runtime: true });
    for (const kind of ['embedding', 'runtime'])
      originals.push(await inspect(join(profile, 'embedding', pins[kind].filename), pins[kind]));
    const manifest = JSON.parse(await readFile(join(profile, 'embedding', 'runtime.json'), 'utf8'));
    assert.equal(manifest.version, 'b11960');
    assert.equal(manifest.target, 'windows-x86_64');
    assert(/^runtime-b11960-[0-9a-f]+$/.test(manifest.directory));
    const runtimeDirectory = join(profile, 'embedding', manifest.directory);
    const serverPath = join(runtimeDirectory, pins.server.filename);
    await inspect(serverPath, pins.server);
    serverFiles = await runtimeInventory(runtimeDirectory);
    result.runtimeFiles = serverFiles;
    serverLog = openSync(join(evidence, 'generation-server.log'), 'wx');
    const args = [
      '-m',
      inputs.generation,
      '--host',
      '127.0.0.1',
      '--port',
      String(port),
      '--alias',
      'qwen3-local-qa',
      '--ctx-size',
      '8192',
      '--parallel',
      '1',
      '--threads',
      '4',
      '--threads-batch',
      '4',
      '--n-gpu-layers',
      '0',
      '--batch-size',
      '512',
      '--ubatch-size',
      '128',
      '--no-mmproj',
      '--no-webui',
      '--cache-ram',
      '0',
      '--poll',
      '0',
      '--jinja',
      '--reasoning',
      'off',
    ];
    const environment = Object.fromEntries(
      Object.entries(process.env).filter(([key]) => !/^(LLAMA_|GGML_|HF_|HUGGINGFACE_)/.test(key)),
    );
    server = spawn(serverPath, args, {
      windowsHide: true,
      cwd: runtimeDirectory,
      env: environment,
      stdio: ['ignore', serverLog, serverLog],
    });
    server.on('error', (error) => {
      spawnError = error;
    });
    result.server = { pid: server.pid, path: serverPath, args };
    await until(
      async () => {
        check();
        try {
          return (
            await fetch(`http://127.0.0.1:${port}/health`, { signal: AbortSignal.timeout(2000) })
          ).ok;
        } catch {
          return false;
        }
      },
      'actual Qwen generation server ready',
      90_000,
    );
    await instance.invoke('model_start');
    await until(
      async () => {
        check();
        const status = await instance.invoke('content_status');
        result.content = status;
        return (
          status.phase === 'ready' &&
          status.indexed === 3 &&
          status.embedded >= 3 &&
          !status.embeddingError
        );
      },
      'actual EmbeddingGemma Q8 plus SQLite/Lance ready',
      180_000,
    );
    result.embedding = await instance.invoke('model_status');
    assert.equal(result.embedding.phase, 'running');
    const { main, invoke } = instance;
    stage = 'actual model RAG chat';
    await main.locator('nav.main-tabs').getByRole('button', { name: '채팅', exact: true }).click();
    const before = new Set(
      (await invoke('rag_list', { query: '', limit: 100, project: null })).map((item) => item.id),
    );
    await main.getByRole('button', { name: '새 대화', exact: true }).click();
    let conversation;
    await until(async () => {
      const created = (await invoke('rag_list', { query: '', limit: 100, project: null })).filter(
        (item) => !before.has(item.id),
      );
      if (created.length !== 1) return false;
      conversation = created[0].id;
      return true;
    }, 'one explicitly created conversation');
    await main
      .locator('.chat-composer textarea')
      .fill(
        '은하 워크숍의 개최일, 장소, 예산 총액, 참가 인원을 자료에서 찾아 한국어로 알려줘. 예약 번호가 자료에 없으면 없다고 답해.',
      );
    await main.getByRole('button', { name: '질문 전송', exact: true }).click();
    await until(
      async () => {
        check();
        const page = await invoke('rag_conversation', {
          id: conversation,
          beforeSequence: null,
          limit: 100,
        });
        const answer = page.turns.find((turn) => turn.role === 'assistant');
        if (answer?.status === 'failed') throw new Error(JSON.stringify(answer.error));
        result.chatAnswer = answer;
        return answer?.status === 'complete';
      },
      'actual model-backed answer persisted',
      200_000,
    );
    result.actualInference = true;
    assertFacts(result.chatAnswer.text);
    assert(result.chatAnswer.usage?.completion_tokens > 0);
    assert.equal(result.chatAnswer.claimsVerified, false);
    assert(
      result.chatAnswer.citations.some(
        (citation) =>
          citation.isCited &&
          pathKey(citation.source.filePath) === pathKey(join(sources, '은하 워크숍.md')),
      ),
    );
    await main.screenshot({ path: join(evidence, 'actual-rag-answer.png') });

    stage = 'enable local document schema through settings';
    await main.getByRole('button', { name: 'AI·모델 설정 열기', exact: true }).click();
    await main
      .getByRole('checkbox', { name: '로컬 문서 생성에 JSON 스키마 사용', exact: true })
      .check();
    await main
      .locator('.settings-modal')
      .getByRole('button', { name: '저장', exact: true })
      .click();
    await main.locator('.settings-modal').waitFor({ state: 'hidden' });
    assert.equal((await invoke('get_settings')).generation.localStructuredDocuments, true);
    stage = 'actual model document outline';
    await main.locator('nav.main-tabs').getByRole('button', { name: '문서', exact: true }).click();
    await main.getByRole('button', { name: '새 문서', exact: true }).click();
    await main.locator('.doc-create input[id$="-new-title"]').fill('은하 워크숍 안내문');
    await main
      .locator('.doc-create textarea[id$="-purpose"]')
      .fill('은하 워크숍 행사 안내문. 확정된 일시와 장소, 예산 및 인원을 간결한 두 절로 정리한다.');
    await main.getByRole('button', { name: '초안 만들기', exact: true }).click();
    await main.locator('.doc-editor h2').filter({ hasText: '은하 워크숍 안내문' }).waitFor();
    const docs = await invoke('doc_list', { query: '은하 워크숍 안내문', limit: 100 });
    assert.equal(docs.length, 1);
    const id = docs[0].id;
    const page = () => invoke('doc_get', { id, revisionId: null });
    result.created = await page();
    const waitOperation = async (operation) =>
      until(
        async () => {
          check();
          const current = await page();
          result.lastDocument = current;
          const job = await invoke('doc_job', { id });
          if (job && ['failed', 'cancelled'].includes(job.phase)) {
            result.failedJob = job;
            const previous = operation === 'body' ? result.reviewed : result.created;
            result.failedJobPreservedRevision = current.revision.id === previous.revision.id;
            assert(
              result.failedJobPreservedRevision,
              'A failed generation changed the current revision',
            );
            throw new Error(JSON.stringify(job));
          }
          return current.revision.operation === operation;
        },
        `actual model ${operation} persisted`,
        200_000,
      );
    const options = main.locator('.doc-options');
    if (!(await options.evaluate((node) => node.open))) await options.locator('summary').click();
    await options
      .locator('textarea[id$="-instruction"]')
      .fill(
        '은하 워크숍 안내문을 한국어로 작성하세요. 일정과 장소, 예산과 참가 인원을 원문 그대로 안내하고 출처를 표시하세요. 자료에 없는 예약 번호를 만들지 마세요.',
      );
    await main.getByRole('button', { name: 'AI로 개요 생성', exact: true }).click();
    await waitOperation('outline');
    result.outline = await page();
    assert.equal(result.outline.revision.outlineReviewed, false);
    await main.getByRole('tab', { name: '본문 블록', exact: true }).click();
    assert(
      await main.getByRole('button', { name: '검토한 개요로 본문 생성', exact: true }).isDisabled(),
    );
    await main.getByRole('tab', { name: '개요', exact: true }).click();
    // Human review is a required product step. Edit only structure/instructions,
    // never insert expected dates, prices, places or generated body into the plan.
    while (await main.locator('.doc-outline ol li').count())
      await main
        .locator('.doc-outline ol li')
        .first()
        .getByRole('button', { name: '항목 삭제', exact: true })
        .click();
    for (const [heading, instruction] of [
      [
        '일정과 장소',
        '원문에 적힌 행사 개최일과 장소를 한국어로 간결하게 안내하고 출처를 표기하세요.',
      ],
      [
        '예산과 참가 인원',
        '원문의 예산 총액과 참가 인원을 항목과 값의 두 열을 가진 표로 정리하세요. 숫자와 단위를 그대로 유지하고 출처를 표기하세요. 예약 번호가 원문에 없으면 자료에 없다고 명시하고 만들지 마세요.',
      ],
    ]) {
      await main
        .locator('.doc-outline')
        .getByRole('button', { name: '개요 항목 추가', exact: true })
        .click();
      const row = main.locator('.doc-outline ol li').last();
      await row.locator('input').fill(heading);
      await row.locator('textarea').fill(instruction);
    }
    await main.locator('.doc-outline input[type=checkbox]').check();
    await main.getByRole('button', { name: '검토한 개요 저장', exact: true }).click();
    await until(async () => (await page()).revision.outlineReviewed, 'explicit review saved');
    result.reviewed = await page();
    assert.equal(result.reviewed.revision.plan.sections.length, 2);
    stage = 'actual schema-constrained body';
    await main.getByRole('tab', { name: '본문 블록', exact: true }).click();
    await main.getByRole('button', { name: '검토한 개요로 본문 생성', exact: true }).click();
    await waitOperation('body');
    result.body = await page();
    const expectedBlockText = result.body.revision.blocks.map((block) => block.markdown);
    await until(async () => {
      const editorText = await main
        .locator('.doc-block textarea')
        .evaluateAll((inputs) => inputs.map((input) => input.value));
      return JSON.stringify(editorText) === JSON.stringify(expectedBlockText);
    }, 'actual generated Markdown displayed in the block editors');
    const hwpxPath = join(evidence, 'actual-document.hwpx');
    await invoke('doc_export_hwpx', {
      id,
      revision: result.body.revision.id,
      path: hwpxPath,
      request: randomUUID(),
      includeSources: true,
      includeTypeMarkers: true,
    });
    result.bodyHwpx = inspectHwpx(hwpxPath);
    result.bodyTables = result.bodyHwpx.tableRows;
    assertFacts(result.body.revision.markdown);
    assert(
      result.bodyTables.some(
        (rows) =>
          rows.length >= 3 &&
          hasTableHeaders(rows, ['항목', '값']) &&
          rows.every((cells) => cells.length === 2) &&
          rows.some((cells) => /347000원/.test(cells.join('').replace(/[\s,]/g, ''))) &&
          rows.some((cells) => /7명/.test(cells.join('').replace(/\s/g, ''))),
      ),
      'The reviewed plan requires 항목/값 columns with the original budget and attendance',
    );
    assert.equal(result.body.revision.claimsVerified, false);
    assert(result.body.revision.provenance.length > 0);
    const sectionIds = new Set(result.reviewed.revision.plan.sections.map((section) => section.id));
    assert(result.body.revision.blocks.length > 0);
    assert(
      result.body.revision.blocks.every(
        (block) => !block.claimsVerified && (!block.sectionId || sectionIds.has(block.sectionId)),
      ),
    );
    assert.deepEqual(
      new Set(result.body.revision.blocks.map((block) => block.sectionId).filter(Boolean)),
      sectionIds,
    );
    assert(
      result.body.revision.citations.some(
        (citation) =>
          pathKey(citation.source.filePath) === pathKey(join(sources, '은하 워크숍.md')),
      ),
    );
    const prior = await invoke('doc_get', { id, revisionId: result.reviewed.revision.id });
    assert.deepEqual(
      prior.revision,
      result.reviewed.revision,
      'Generation must not mutate the reviewed revision',
    );
    const exported = await invoke('doc_export', {
      id,
      revision: result.body.revision.id,
      includeSources: true,
      includeTypeMarkers: true,
    });
    assertFacts(exported);
    await writeFile(join(evidence, 'actual-document.md'), exported);
    await main.screenshot({ path: join(evidence, 'actual-document-body.png') });
    await main.setViewportSize({ width: 760, height: 900 });
    result.narrowLayout = await main.evaluate(() => ({
      width: innerWidth,
      scroll: document.body.scrollWidth,
    }));
    assert(result.narrowLayout.scroll <= result.narrowLayout.width + 1);
    await main.screenshot({ path: join(evidence, 'actual-document-narrow.png') });
    assert.deepEqual(instance.errors, []);
    result.firstSessionErrors = [...instance.errors];
    stage = 'reopen actual generated document and answer';
    await shutdown(instance);
    assert.equal(child.exitCode, 0);
    instance = undefined;
    child = undefined;
    await launchFixture();
    assert.equal((await instance.invoke('get_settings')).generation.localStructuredDocuments, true);
    assert.equal(
      (await instance.invoke('model_status')).pid,
      null,
      'Reopen must not automatically start inference',
    );
    const reopened = await instance.invoke('doc_get', { id, revisionId: null });
    assert.deepEqual(reopened.revision, result.body.revision);
    const chat = await instance.invoke('rag_conversation', {
      id: conversation,
      beforeSequence: null,
      limit: 100,
    });
    assert.equal(
      chat.turns.find((turn) => turn.id === result.chatAnswer.id).text,
      result.chatAnswer.text,
    );
    assert.deepEqual(instance.errors, []);
    result.reopened = {
      documentRevisionId: reopened.revision.id,
      assistantTurnId: result.chatAnswer.id,
    };
  }
  check();
  result.passed = true;
} catch (error) {
  result.error =
    (child || instance) && !verifiedProfile
      ? 'Portable profile not verified; no model, generation, screenshot or cleanup IPC sent'
      : String(error.stack ?? error);
  if (instance && verifiedProfile)
    await instance.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
  throw new Error(result.error);
} finally {
  // Never send model_stop or quit to an endpoint without our unique scope nonce.
  if (instance && verifiedProfile)
    await shutdown(instance).catch((error) => {
      result.shutdownError = String(error);
    });
  else if (instance) await instance.browser.close().catch(() => {});
  stopOwned();
  for (const [name, owned] of [
    ['app', child],
    ['server', server],
  ]) {
    if (owned && owned.pid)
      await until(() => !alive(owned), `owned ${name} cleanup`, 10_000).catch((error) => {
        result.shutdownError = String(error);
      });
  }
  clearInterval(guard);
  process.off('SIGINT', interrupt);
  process.off('SIGTERM', interrupt);
  if (serverLog !== undefined) closeSync(serverLog);
  if (verifiedProfile && result.failedJob && child && !alive(child)) {
    try {
      result.failedDocumentResponse = documentFailureEvidence(
        join(directory, 'app', 'PrismData'),
        result.failedJob,
      );
    } catch (error) {
      result.documentEvidenceError = String(error);
    }
  }
  result.stage = stage;
  result.profileVerified = verifiedProfile;
  result.memory = {
    minimumObserved: memorySamples ? minimumAvailable : null,
    samples: memorySamples,
    after: freemem(),
    resourceStop,
  };
  result.errors = verifiedProfile ? (instance?.errors ?? []) : [];
  result.warnings = verifiedProfile ? warnings : [];
  result.appExitCode = child?.exitCode ?? null;
  result.serverExitCode = server?.exitCode ?? null;
  result.serverExitSignal = server?.signalCode ?? null;
  // Processes have ended. Rehash without liveness checks, preserving failures.
  child = undefined;
  server = undefined;
  const integrity = [];
  for (const original of [...originals, ...(serverFiles ?? [])]) {
    try {
      integrity.push({
        ...original,
        unchanged: (await inspect(original.path, undefined, false)).sha256 === original.sha256,
      });
    } catch {
      integrity.push({ ...original, unchanged: false });
    }
  }
  result.integrity = integrity;
  let finalFailure;
  if (
    result.passed &&
    (result.shutdownError ||
      interrupted ||
      resourceStop ||
      integrity.some((item) => !item.unchanged) ||
      (!verifyOnly && result.appExitCode !== 0))
  ) {
    result.passed = false;
    finalFailure = new Error(
      'Model checks completed but cleanup, resource or input integrity failed',
    );
    result.error = String(finalFailure);
  }
  await writeFile(join(evidence, 'result.json'), JSON.stringify(result, null, 2));
  console.log(
    JSON.stringify({
      passed: result.passed,
      verificationOnly: verifyOnly,
      actualInference: result.actualInference,
      stage,
      evidence,
      resourceStop,
      error: result.error,
    }),
  );
  if (finalFailure) throw finalFailure;
}
