// Real bundled WebView -> Rust extraction/SQLite -> EmbeddingGemma 2/LanceDB.
// Only generated documents and isolated app data; no user clipboard or input injection.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile, rename, unlink, symlink } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-content-'));
const folder = join(directory, '문서');
const privateFolder = join(directory, '파일명만');
const evidence = join(tmpdir(), 'prism-ui-evidence');
await Promise.all(
  [folder, privateFolder, evidence].map((path) => mkdir(path, { recursive: true })),
);
const travel = join(folder, '제주 출장.md');
await writeFile(travel, '# 제주도 출장\n제주도 출장 경비: 항공료 120000원, 숙박비 80000원.');
await writeFile(join(folder, '정원.md'), '# 정원 관리\n토마토와 오이 재배 및 화분 물주기.');
await writeFile(join(folder, '회의.md'), '# 회의록\n소프트웨어 개발 일정과 배포 계획.');
await writeFile(join(folder, '손상.pdf'), '%PDF-1.7 broken synthetic fixture');
await writeFile(join(privateFolder, '비공개.md'), 'privatecanary secret');
await symlink(privateFolder, join(folder, '외부연결'), 'junction');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    shortcut: 'Ctrl+Alt+Shift+F9',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'public',
        name: '본문 테스트',
        path: folder,
        filenameEnabled: true,
        contentEnabled: false,
      },
      {
        id: 'private',
        name: '파일명만',
        path: privateFolder,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
let instance;
try {
  instance = await launch(directory);
  let { main, quick, invoke } = instance;
  const input = main.getByRole('textbox', { name: '검색어', exact: true });
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await main.getByRole('heading', { name: '이 범위에는 본문·AI 검색 폴더가 없습니다.' }).waitFor();
  await main.getByRole('button', { name: '검색 폴더 설정', exact: true }).click();
  const dialog = main.getByRole('dialog');
  const scope = dialog.locator('.scope-setting').filter({ hasText: '본문 테스트' });
  await scope.getByRole('checkbox').nth(1).check();
  await dialog.getByRole('button', { name: '저장', exact: true }).click();
  await until(async () => {
    const status = await invoke('content_status');
    return status.phase === 'ready' && status.indexed === 3;
  }, 'only authorized documents indexed');
  const status = await invoke('content_status');
  assert(status.issues.some((issue) => issue.path.endsWith('손상.pdf')));
  await main.getByRole('button', { name: '자동', exact: true }).click();
  await input.fill('120000');
  await main
    .locator('.automatic-search-notice')
    .filter({ hasText: '자연어 모델이 실행 중이 아닙니다.' })
    .waitFor();
  await main.getByRole('option').filter({ hasText: '제주 출장.md' }).waitFor();
  assert.equal(
    (await invoke('model_status')).phase,
    'notInstalled',
    'Automatic search must not install or start a model',
  );
  await input.fill(Array.from({ length: 33 }, (_, index) => `autoword${index}`).join(' '));
  await main
    .getByRole('heading', { name: '키워드 검색을 실행하지 못했습니다.', exact: true })
    .waitFor();
  await input.fill('제주 출장');
  await main
    .locator('.file-result-list')
    .getByRole('option')
    .filter({ hasText: '제주 출장.md' })
    .waitFor();
  assert.equal(
    await main.locator('.content-panel').count(),
    0,
    'Filename matches remain the fast primary result',
  );
  await input.fill('ext:pdf 없는항목');
  await until(
    async () => (await invoke('get_search_session')).query === 'ext:pdf 없는항목',
    'file filter query saved',
  );
  assert.equal(
    await main.locator('.content-panel').count(),
    0,
    'Explicit filename filters never fall back to body search',
  );
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await input.fill('privatecanary');
  await main
    .getByRole('heading', { name: '일치하는 본문을 찾지 못했습니다.', exact: true })
    .waitFor();
  await input.fill('제주도 경비');
  await main.getByRole('option').filter({ hasText: '제주 출장.md' }).waitFor();
  assert((await main.locator('.content-results').innerText()).includes('120000'));
  await main.screenshot({ path: join(evidence, 'content-main.png') });
  await writeFile(travel, '# 제주도 출장\n제주도 출장 경비 변경: 항공료 130000원. liveupdated');
  await input.fill('liveupdated');
  await main.getByRole('option').filter({ hasText: '제주 출장.md' }).waitFor();
  const renamed = join(folder, '변경된 출장.md');
  await rename(travel, renamed);
  await main.getByRole('option').filter({ hasText: '변경된 출장.md' }).waitFor();
  await main.getByRole('button', { name: '자연어', exact: true }).click();
  await input.fill('제주도 출장을 가려면 돈이 얼마나 들까?');
  await main.locator('.content-search-error').waitFor();
  await main
    .locator('.content-search-error')
    .getByRole('button', { name: 'AI·모델 설정', exact: true })
    .click();
  await main.locator('.embedding-panel').getByText('미설치', { exact: true }).first().waitFor();
  await main.screenshot({ path: join(evidence, 'model-not-installed.png') });
  await main.getByRole('dialog').getByRole('button', { name: '취소', exact: true }).click();
  await invoke('model_import', {
    path: resolve('.dev/models/embeddinggemma-2-Q8_0.gguf'),
    runtime: false,
  });
  await invoke('model_import', {
    path: resolve('.dev/runtime/llama-b11960-bin-win-cpu-x64.zip'),
    runtime: true,
  });
  await main
    .locator('.content-search-error')
    .getByRole('button', { name: 'AI·모델 설정', exact: true })
    .click();
  await main.getByRole('button', { name: '모델 시작', exact: true }).click();
  await until(
    async () => (await invoke('model_status')).phase === 'running',
    'real local CPU model starts',
    120_000,
  );
  await until(
    async () => {
      const status = await invoke('content_status');
      return status.phase === 'ready' && status.embedded === 3 && !status.embeddingError;
    },
    'real document vectors committed to LanceDB',
    120_000,
  );
  await main.getByRole('dialog').getByRole('button', { name: '취소', exact: true }).click();
  await until(
    async () => (await main.getByRole('option').count()) > 0,
    'semantic results rendered',
    60_000,
  );
  assert((await main.getByRole('option').first().innerText()).includes('변경된 출장.md'));
  // Observe actual IPC responses without replacing the model, stores or transport.
  // Driving the UI avoids racing its window-scoped search cancellation with a
  // second direct invoke for the same window.
  await main.evaluate(() => {
    const original = window.fetch.bind(window);
    window.__hybridEvidence = [];
    window.__automaticEvidence = [];
    window.fetch = async (input, options) => {
      const response = await original(input, options);
      const url = new URL(typeof input === 'string' ? input : (input.url ?? input.href));
      if (
        url.hostname === 'ipc.localhost' &&
        ['/search_hybrid', '/search_automatic'].includes(url.pathname)
      ) {
        void response
          .clone()
          .json()
          .then((value) => {
            if (Array.isArray(value?.hits))
              (url.pathname === '/search_hybrid'
                ? window.__hybridEvidence
                : window.__automaticEvidence
              ).push(value);
          });
      }
      return response;
    };
  });
  await input.fill('출장');
  await until(
    async () =>
      main.evaluate(() =>
        window.__hybridEvidence.some(
          (value) => value.keywordCandidates > 0 && value.semanticCandidates > 0,
        ),
      ),
    'actual keyword and vector branches combined',
    60_000,
  );
  const combined = await main.evaluate(() => window.__hybridEvidence.at(-1));
  assert.equal(combined.keywordSkipped, false);
  assert.equal(new Set(combined.hits.map((hit) => hit.chunk.id)).size, combined.hits.length);
  assert(combined.hits.some((hit) => hit.keywordRank && hit.semanticRank));
  for (const hit of combined.hits) {
    const expected =
      (hit.keywordRank ? 1 / (60 + hit.keywordRank) : 0) +
      (hit.semanticRank ? 1 / (60 + hit.semanticRank) : 0);
    assert(Math.abs(hit.score - expected) < 1e-10);
    assert(!hit.chunk.filePath.includes('파일명만'));
  }
  await main.getByText('키워드·의미 일치', { exact: true }).first().waitFor();
  await main.locator('.content-score').filter({ hasText: '결합 순위 점수' }).first().waitFor();
  await main.screenshot({ path: join(evidence, 'hybrid-main.png'), animations: 'disabled' });
  await main.getByRole('button', { name: '필터', exact: true }).click();
  await main.evaluate(() => {
    window.__hybridEvidence = [];
  });
  await main.getByLabel('확장자', { exact: true }).fill('pdf');
  await until(
    async () =>
      main.evaluate(() =>
        window.__hybridEvidence.some(
          (value) => value.hits.length === 0 && value.keywordCandidates === 0,
        ),
      ),
    'extension filter applies to both hybrid branches',
    60_000,
  );
  assert.equal(await main.getByRole('option').count(), 0);
  await main.getByLabel('확장자', { exact: true }).fill('');
  await main.getByRole('button', { name: '필터', exact: true }).click();
  await main.getByText('키워드·의미 일치', { exact: true }).first().waitFor();
  await main.getByRole('button', { name: 'AI·모델 설정', exact: true }).click();
  const combine = main.getByLabel('자연어 검색에 본문 키워드 결과 결합', { exact: true });
  await combine.uncheck();
  await main.getByRole('dialog').getByRole('button', { name: '취소', exact: true }).click();
  assert.equal((await invoke('get_settings')).hybridSearch, true);
  await main.getByRole('button', { name: 'AI·모델 설정', exact: true }).click();
  await combine.uncheck();
  await main.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await main.locator('.content-score').filter({ hasText: '코사인 유사도' }).first().waitFor();
  assert.equal((await invoke('get_settings')).hybridSearch, false);
  await main.getByRole('button', { name: 'AI·모델 설정', exact: true }).click();
  await combine.check();
  await main.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await main.locator('.content-score').filter({ hasText: '결합 순위 점수' }).first().waitFor();
  await input.fill(Array.from({ length: 33 }, (_, index) => `syntheticword${index}`).join(' '));
  await main
    .getByText(
      '키워드 검색의 입력 한도를 넘어 자연어 검색 순위만 표시합니다. 검색어를 줄이면 두 결과를 결합할 수 있습니다.',
      { exact: true },
    )
    .waitFor();
  await input.fill('제주도 출장을 가려면 돈이 얼마나 들까?');
  await until(
    async () => (await main.getByRole('option').first().innerText()).includes('변경된 출장.md'),
    'hybrid results return to the requested Korean question',
    60_000,
  );
  await main.getByRole('button', { name: '자동', exact: true }).click();
  await until(
    async () =>
      main.evaluate(() =>
        window.__automaticEvidence.some(
          (value) => value.semanticState === 'used' && value.ranking === 'hybrid',
        ),
      ),
    'automatic routing uses the real running model',
    60_000,
  );
  await main.getByRole('option').filter({ hasText: '변경된 출장.md' }).first().waitFor();
  await main.screenshot({ path: join(evidence, 'automatic-main.png'), animations: 'disabled' });
  await input.fill('변경된 출장.md');
  await main
    .locator('.file-result-list')
    .getByRole('option')
    .filter({ hasText: '변경된 출장.md' })
    .waitFor();
  assert.equal(await main.locator('.content-panel').count(), 0);
  await input.fill('130000');
  await main.locator('.automatic-search-notice').waitFor();
  await main.getByRole('button', { name: '필터', exact: true }).click();
  await main.getByLabel('확장자', { exact: true }).fill('pdf,md');
  await main.getByRole('option').filter({ hasText: '변경된 출장.md' }).first().waitFor();
  await main.getByRole('button', { name: '필터', exact: true }).click();
  const openAutoSettings = async () => {
    await main.getByRole('button', { name: '설정', exact: true }).click();
    await main.getByRole('button', { name: 'AI·모델', exact: true }).click();
  };
  await openAutoSettings();
  await main.getByLabel('자동 모드에서 자연어 검색 사용', { exact: true }).uncheck();
  await main.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await main
    .locator('.automatic-search-notice')
    .filter({ hasText: '설정에서 꺼져 있습니다.' })
    .waitFor();
  await main.getByRole('option').filter({ hasText: '변경된 출장.md' }).first().waitFor();
  assert.equal((await invoke('model_status')).phase, 'running');
  await openAutoSettings();
  await main.getByLabel('자동 모드에서 자연어 검색 사용', { exact: true }).check();
  await main.getByLabel('자연어 검색에 본문 키워드 결과 결합', { exact: true }).uncheck();
  await main.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await until(
    async () => main.evaluate(() => window.__automaticEvidence.at(-1)?.ranking === 'cosine'),
    'automatic mode respects semantic-only ranking',
    60_000,
  );
  await openAutoSettings();
  await main.getByLabel('자연어 검색에 본문 키워드 결과 결합', { exact: true }).check();
  await main.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await input.fill('제주도 출장을 가려면 돈이 얼마나 들까?');
  await main
    .locator('.automatic-search-notice')
    .filter({ hasText: '로컬 자연어 모델로 검색했습니다.' })
    .waitFor();
  await main.getByRole('button', { name: '필터', exact: true }).click();
  await main.getByLabel('확장자', { exact: true }).fill('');
  await main.getByRole('button', { name: '필터', exact: true }).click();
  await main.screenshot({ path: join(evidence, 'semantic-main.png') });
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  assert.equal(
    await quick.getByRole('dialog').count(),
    0,
    'Main settings events must not open a hidden Spotlight dialog',
  );
  await until(
    async () => (await quick.getByRole('option').count()) > 0,
    'Spotlight semantic handoff',
    60_000,
  );
  await quick.screenshot({ path: join(evidence, 'semantic-quick.png') });
  await quick.getByRole('button', { name: 'AI·모델 설정', exact: true }).click();
  await main.getByRole('dialog').waitFor();
  assert.equal(
    await quick.getByRole('dialog').count(),
    0,
    'Spotlight settings requests open only the main window',
  );
  await main.getByRole('button', { name: '모델 정지', exact: true }).click();
  await until(
    async () => (await invoke('model_status')).phase === 'ready',
    'owned embedding server stops',
  );
  await main.getByRole('dialog').getByRole('button', { name: '취소', exact: true }).click();
  await main.getByRole('button', { name: '본문', exact: true }).click();
  await input.fill('liveupdated');
  await main.getByRole('option').filter({ hasText: '변경된 출장.md' }).waitFor();
  await unlink(renamed);
  await main
    .getByRole('heading', { name: '일치하는 본문을 찾지 못했습니다.', exact: true })
    .waitFor();
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = await launch(directory);
  assert.equal(
    (await instance.invoke('model_status')).phase,
    'ready',
    'model must not auto-start after restart',
  );
  await until(
    async () => (await instance.invoke('content_status')).phase === 'ready',
    'persisted content reopen',
  );
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      evidence,
      model: 'google/embeddinggemma-2',
      source: 'native LanceDB',
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
