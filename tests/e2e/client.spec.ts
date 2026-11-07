import { expect, test } from '@playwright/test';
import { mkdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const evidence = join(tmpdir(), 'prism-ui-evidence');
test.beforeAll(async () => {
  await mkdir(evidence, { recursive: true });
});
test.beforeEach(async ({ page }) => {
  // These are established-profile regressions; first-run setup has its own spec.
  await page.addInitScript(() => {
    if (!localStorage.getItem('prism.settings.v1'))
      localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 }));
  });
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  (page as unknown as { appErrors: string[] }).appErrors = errors;
});
test.afterEach(async ({ page }) => {
  expect((page as unknown as { appErrors: string[] }).appErrors).toEqual([]);
});

test('renders the empty library without fabricated files', async ({ page }) => {
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page.getByRole('heading', { name: '검색할 폴더를 추가하세요' })).toBeVisible();
  await expect(page.getByText('브라우저 미리보기', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '본문', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: '폴더 추가', exact: true }).last().click();
  await expect(page.getByRole('status')).toContainText('데스크톱 앱');
  await page.getByRole('status').getByRole('button').click();
  await page.screenshot({ path: join(evidence, 'main.png') });
});

test('clipboard command stays native-only in browser preview and empty quick stays compact', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: '클립보드 기록', exact: true }).click();
  await expect(page.getByRole('textbox', { name: '검색어', exact: true })).toHaveValue('clip:');
  await expect(page.locator('.clipboard-panel')).toContainText('데스크톱 앱');
  await expect(page.locator('.clipboard-notice')).toHaveCount(0);
  await page.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await expect(page.locator('.clipboard-panel')).toContainText('데스크톱 앱');
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill(' ');
  await expect(page.locator('.quick-results')).toHaveCount(0);
  await expect(page.locator('.quick-empty')).toHaveCount(0);
});

test('content and semantic modes persist without fabricated browser results', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '본문', exact: true }).click();
  await expect(page.locator('.content-panel')).toContainText('데스크톱');
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('제주 출장 경비');
  await page.getByRole('button', { name: '자연어', exact: true }).click();
  await expect(input).toHaveValue('제주 출장 경비');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.getByRole('main', { name: '빠른 검색', exact: true })).toBeVisible();
  await expect(input).toBeFocused();
  await expect(page.locator('.content-panel')).toContainText('데스크톱');
  await input.press('Control+Enter');
  await expect(page.getByRole('button', { name: '자연어', exact: true })).toHaveClass(/active/);
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByRole('button', { name: 'AI·모델', exact: true }).click();
  await expect(page.locator('.embedding-panel')).toContainText('데스크톱');
  await page.getByRole('button', { name: '취소', exact: true }).click();
});

test('transfers query and filters to quick search and handles IME Enter', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('개발 계획');
  await page.getByRole('button', { name: '필터', exact: true }).click();
  await page.getByLabel('확장자', { exact: true }).fill('hwpx');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.getByRole('main', { name: '빠른 검색' })).toBeVisible();
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await expect(input).toHaveValue('개발 계획');
  await input.dispatchEvent('keydown', {
    key: 'Enter',
    code: 'Enter',
    isComposing: true,
    keyCode: 229,
  });
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.session.v1') ?? '{}').recentQueries ?? [],
      ),
    )
    .toEqual([]);
  await input.press('Enter');
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('prism.session.v1') ?? '{}').recentQueries,
      ),
    )
    .toEqual(['개발 계획']);
  await input.press('Control+Enter');
  await expect(page.getByRole('heading', { name: '파일 검색', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '필터', exact: true }).click();
  await expect(page.getByLabel('확장자', { exact: true })).toHaveValue('hwpx');
  await page.getByRole('button', { name: '최근 검색', exact: true }).click();
  await expect(page.getByRole('button', { name: '개발 계획 ↗' })).toBeVisible();
});

test('persists theme, language and text size; cancellation does not apply changes', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('테마', { exact: true }).selectOption('dark');
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('테마', { exact: true }).selectOption('dark');
  await page.getByLabel('언어', { exact: true }).selectOption('en');
  await page.getByLabel('글자 크기', { exact: true }).fill('16');
  await page.getByLabel('한·영 키보드 오입력 보정', { exact: true }).uncheck();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'File search', exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('html')).toHaveCSS('font-size', '16px');
  expect(
    await page.evaluate(
      () => JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').keyboardCorrection,
    ),
  ).toBe(false);
  await page.screenshot({ path: join(evidence, 'dark.png') });
});

test('keeps filename and AI scopes independent', async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem(
      'prism.settings.v1',
      JSON.stringify({
        schemaVersion: 1,
        revision: 0,
        scopes: [
          {
            id: 'test-scope',
            name: '테스트 문서',
            path: 'C:\\PrismTest',
            filenameEnabled: true,
            contentEnabled: false,
          },
        ],
      }),
    ),
  );
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByRole('button', { name: '검색 범위', exact: true }).click();
  await expect(page.getByLabel('이 폴더 본문 외부 AI 전송 허용', { exact: true })).toBeDisabled();
  await page.getByLabel('본문·AI 색인', { exact: true }).check();
  await page.getByLabel('이 폴더 본문 외부 AI 전송 허용', { exact: true }).check();
  await page.getByLabel('본문·AI 색인', { exact: true }).uncheck();
  await page.getByLabel('본문·AI 색인', { exact: true }).check();
  await expect(
    page.getByLabel('이 폴더 본문 외부 AI 전송 허용', { exact: true }),
  ).not.toBeChecked();
  await expect(page.getByLabel('파일명 검색', { exact: true })).toBeChecked();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').scopes[0].contentEnabled,
      ),
    )
    .toBe(true);
  await expect(page.getByRole('heading', { name: '검색 범위가 준비됐어요' })).toBeVisible();
  await expect(page.getByText('파일명을 입력하세요', { exact: true })).toBeVisible();
});

test('saves generation connection preferences without contacting a provider or storing keys', async ({
  page,
}) => {
  const providerRequests: string[] = [];
  page.on('request', (request) => {
    if (request.url().startsWith('https://api.openai.com')) providerRequests.push(request.url());
  });
  await page.goto('/');
  await expect(page).toHaveTitle('Prism');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  const settings = async () => {
    await page.getByRole('button', { name: '설정', exact: true }).click();
    await page.getByRole('button', { name: 'AI·모델', exact: true }).click();
  };
  await settings();
  const panel = page.locator('.generation-panel');
  await expect(panel.getByLabel('사용 모드', { exact: true })).toHaveValue('local');
  await expect(panel.getByLabel('실패 시 다른 서버 사용 허용', { exact: true })).not.toBeChecked();
  await expect(panel.getByLabel('외부 서버에 질문 전송 허용', { exact: true })).not.toBeChecked();
  await panel.getByLabel('사용 모드', { exact: true }).selectOption('auto');
  await panel.getByLabel('자동 선택 우선순위', { exact: true }).selectOption('externalFirst');
  await panel.getByLabel('외부 서버에 질문 전송 허용', { exact: true }).check();
  const external = panel.getByRole('group', { name: '외부 서버', exact: true });
  await external.getByLabel('이 서버 연결 설정', { exact: true }).check();
  await external.getByRole('button', { name: 'OpenAI', exact: true }).click();
  await external.getByLabel('모델 이름', { exact: true }).fill('synthetic-user-chosen-model');
  await expect(external.getByLabel('토큰 제한 파라미터', { exact: true })).toHaveValue(
    'maxCompletionTokens',
  );
  await expect(
    external.getByText('API 키 상태 확인·저장은 데스크톱 앱에서만 사용할 수 있습니다.', {
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: '취소', exact: true }).click();
  await settings();
  await expect(panel.getByLabel('사용 모드', { exact: true })).toHaveValue('local');
  await expect(external.getByLabel('이 서버 연결 설정', { exact: true })).not.toBeChecked();
  const local = panel.getByRole('group', { name: '로컬 서버', exact: true });
  await local.getByLabel('이 서버 연결 설정', { exact: true }).check();
  await local.getByRole('button', { name: 'LM Studio', exact: true }).click();
  await local.getByLabel('모델 이름', { exact: true }).fill('synthetic-local-model');
  const withoutKey = local.getByLabel('로컬 서버에 API 키 없이 연결', { exact: true });
  const structured = local.getByLabel('로컬 문서 생성에 JSON 스키마 사용', { exact: true });
  await expect(structured).not.toBeChecked();
  await structured.check();
  await expect(external.getByLabel('로컬 문서 생성에 JSON 스키마 사용')).toHaveCount(0);
  await expect(withoutKey).not.toBeChecked();
  await withoutKey.check();
  await expect(local.locator('.generation-credentials')).toHaveCount(0);
  await expect(local.getByText(/키는 아래 삭제 버튼/)).toHaveCount(0);
  await expect(local.getByLabel('서버 URL', { exact: true })).toHaveValue(
    'http://127.0.0.1:1234/v1',
  );
  await panel.getByLabel('문맥 최대 문자 수', { exact: true }).fill('12000');
  await page.getByRole('dialog').getByRole('button', { name: '저장', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').generation?.local?.model,
      ),
    )
    .toBe('synthetic-local-model');
  const saved = await page.evaluate(
    () => JSON.parse(localStorage.getItem('prism.settings.v1') ?? '{}').generation,
  );
  expect(saved).toMatchObject({
    mode: 'local',
    fallback: false,
    allowExternalQuestions: false,
    localWithoutApiKey: true,
    localStructuredDocuments: true,
    maxContextChars: 12000,
    external: null,
    local: { temperature: null, tokenParameter: 'maxTokens' },
  });
  expect(JSON.stringify(saved)).not.toMatch(/apiKey|secret|Bearer/);
  expect(providerRequests).toEqual([]);
  await settings();
  await expect(structured).toBeChecked();
  await expect(withoutKey).toBeChecked();
  await withoutKey.uncheck();
  await expect(local.locator('.generation-credentials')).toHaveCount(1);
  await expect(local.getByText(/키는 아래 삭제 버튼/)).toBeVisible();
  await withoutKey.check();
  await panel.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(evidence, 'generation-settings.png') });
  await page.setViewportSize({ width: 760, height: 900 });
  await structured.scrollIntoViewIfNeeded();
  await expect(structured).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
  ).toBe(true);
  await page.screenshot({ path: join(evidence, 'generation-settings-narrow.png') });
});

test('edits, persists and exports a real Markdown draft', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '문서', exact: true }).click();
  await page.getByRole('textbox', { name: '문서 제목', exact: true }).fill('Prism 구현 메모');
  await page
    .getByRole('textbox', { name: 'Markdown', exact: true })
    .fill('# 첫 단계\n\n- 두 창\n- 설정 저장');
  await page.getByRole('button', { name: '채팅', exact: true }).click();
  await expect(page.getByRole('heading', { name: '문서와 대화', exact: true })).toBeVisible();
  await expect(page.locator('.chat-native-only')).toContainText('데스크톱 앱');
  await expect(page.getByRole('textbox', { name: '질문', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: '문서', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Markdown', exact: true })).toContainText(
    '- 설정 저장',
  );
  await page.getByRole('button', { name: 'AI 문서', exact: true }).click();
  await expect(page.locator('.doc-native')).toContainText('데스크톱 앱');
  await page.getByRole('button', { name: '기존 로컬 메모 열기', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Markdown', exact: true })).toHaveValue(
    '# 첫 단계\n\n- 두 창\n- 설정 저장',
  );
  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Markdown 저장', exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('Prism 구현 메모.md');
  expect(await readFile((await download.path())!, 'utf8')).toContain('# 첫 단계');
  await page.screenshot({ path: join(evidence, 'documents.png') });
});

test('does not retain search history when disabled', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('이전 검색 상태 유지', { exact: true }).uncheck();
  await page.getByRole('button', { name: '저장', exact: true }).click();
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('임시 검색');
  await page.getByRole('textbox', { name: '검색어', exact: true }).press('Enter');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.getByRole('textbox', { name: '검색어', exact: true })).toHaveValue('');
  await page.reload();
  expect(await page.evaluate(() => localStorage.getItem('prism.session.v1'))).toBeNull();
});

test('shows responsive settings and quick search without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByRole('button', { name: '한국어', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '설정', exact: true })).toBeVisible();
  await page.screenshot({ path: join(evidence, 'settings-mobile.png') });
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await page.screenshot({ path: join(evidence, 'mobile.png') });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('   ');
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.getByRole('textbox', { name: '검색어', exact: true }).fill('검색');
  await expect(page.getByRole('heading', { name: '검색할 폴더가 없어요' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('textbox', { name: '검색어', exact: true }).press('Escape');
  await expect(page.getByRole('heading', { name: '파일 검색', exact: true })).toBeVisible();
});

test('captures desktop reference states', async ({ browser, baseURL }) => {
  // A hand-made context does not inherit the config's baseURL, so pass it on.
  // Then the test follows whichever server is running (dev 1420 or bundled preview 1421).
  const context = await browser.newContext({
    baseURL,
    viewport: { width: 1190, height: 846 },
    deviceScaleFactor: 1.25,
    colorScheme: 'light',
  });
  const page = await context.newPage();
  await page.addInitScript(() =>
    localStorage.setItem('prism.settings.v1', JSON.stringify({ onboardingVersion: 1 })),
  );
  await page.goto('/');
  await expect(page.getByRole('heading', { name: '검색할 폴더를 추가하세요' })).toBeVisible();
  await page.screenshot({ path: join(evidence, 'main-reference.png') });
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.screenshot({ path: join(evidence, 'settings.png') });
  await page.getByRole('button', { name: '취소', exact: true }).click();
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.locator('.quick-empty')).toHaveCount(0);
  await page.screenshot({ path: join(evidence, 'quick.png') });
  await context.close();
});

test('calculates in a worker, converts units, reports errors and discards stale answers', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('= 2+3*4');
  await expect(page.locator('.utility-value')).toHaveText('14', { timeout: 12000 });
  await expect(page.locator('.accuracy-note')).toHaveCount(0);
  await page.getByRole('button', { name: '복사', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('14');
  await page.screenshot({ path: join(evidence, 'calculator-main.png') });
  await input.fill('= 1/3');
  await expect(page.locator('.utility-value')).toHaveText('0.333333333333');
  await expect(page.locator('.exact-value')).toContainText('1/3');
  await expect(page.locator('.accuracy-note')).toHaveText('근삿값 · 유효숫자 12자리로 반올림');
  await page.getByRole('button', { name: /빠른 검색/ }).click();
  await expect(page.locator('.utility-value')).toHaveText('0.333333333333');
  await input.fill('=48/2(9+3)');
  await expect(page.locator('.utility-value')).toHaveText('2');
  await expect(page.locator('.interpreted-expression')).toContainText('48 / (2 * (9 + 3))');
  await page.screenshot({ path: join(evidence, 'implicit-calculation.png') });
  await input.fill('=48/2*(9+3)');
  await expect(page.locator('.utility-value')).toHaveText('288');
  await input.fill('unit: 10인치 to cm');
  await expect(page.locator('.utility-value')).toHaveText('25.4 cm');
  await page.screenshot({ path: join(evidence, 'units-quick.png') });
  await input.fill('unit: 1 GiB to MiB');
  await expect(page.locator('.utility-value')).toHaveText('1024 MiB');
  await input.fill('= 2+');
  await expect(page.locator('.utility-result').getByRole('status')).toContainText('수식을 확인');
  await input.fill('= 9^(9^9)');
  await input.fill('= 8+8');
  await expect(page.locator('.utility-value')).toHaveText('16');
  await input.fill('2+3.txt');
  await expect(page.locator('.utility-result')).toHaveCount(0);
  await input.fill('');
  await expect(
    page.locator('.quick-empty, .quick-results, .quick-meta, .quick-footer'),
  ).toHaveCount(0);
});

test('uses configured degrees and keeps web queries local until explicit execution', async ({
  page,
  context,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: '설정', exact: true }).click();
  await page.getByLabel('각도 단위', { exact: true }).selectOption('deg');
  await page.getByRole('button', { name: '저장', exact: true }).click();
  const input = page.getByRole('textbox', { name: '검색어', exact: true });
  await input.fill('= sin(90)');
  await expect(page.locator('.utility-value')).toHaveText('1', { timeout: 12000 });
  const sent: string[] = [];
  await context.route(/https:\/\/www\.google\.com\/search.*/, async (route) => {
    sent.push(route.request().url());
    await route.fulfill({ body: '<title>Search test</title>' });
  });
  await input.fill('web: 한글 & a=1 #topic');
  await expect(page.locator('.web-query')).toHaveText('한글 & a=1 #topic');
  expect(sent).toEqual([]);
  const popupPromise = context.waitForEvent('page');
  await page.getByRole('button', { name: '브라우저에서 검색', exact: true }).click();
  const popup = await popupPromise;
  await expect.poll(() => sent.length).toBe(1);
  expect(new URL(sent[0]).searchParams.get('q')).toBe('한글 & a=1 #topic');
  await popup.close();
  await page.setViewportSize({ width: 390, height: 844 });
  await input.fill('unit: 20 km/L to L/100km');
  await expect(page.locator('.utility-value')).toHaveText('5 L/100km');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: join(evidence, 'units-mobile.png') });
});
