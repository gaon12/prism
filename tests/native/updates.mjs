// Real native update service/UI and Tauri signature verifier. No installer is executed.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-updates-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
const source = join(directory, 'source');
await mkdir(source);
await mkdir(evidence, { recursive: true });
await writeFile(join(source, 'keep.md'), 'Synthetic file preserved after failed update.');
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    shortcut: 'Ctrl+Alt+Shift+F9',
    startOnLogin: false,
    clipboard: { enabled: false, onboarded: true },
    updates: { autoCheck: true },
    scopes: [
      {
        id: 'source',
        name: '합성 자료',
        path: source,
        filenameEnabled: true,
        contentEnabled: false,
        externalAllowed: false,
      },
      {
        id: 'body',
        name: '합성 앱 폴더',
        path: directory,
        filenameEnabled: false,
        contentEnabled: true,
        externalAllowed: false,
      },
    ],
  }),
);
const fixture = resolve('tests/fixtures/updates');
const publicKey = (await readFile(join(fixture, 'public.pub'), 'utf8')).trim();
const otherKey = (await readFile(join(fixture, 'other-public.pub'), 'utf8')).trim();
const signature = (await readFile(join(fixture, 'payload.bin.sig'), 'utf8')).trim();
const legacy = (await readFile(join(fixture, 'legacy.bin.sig'), 'utf8')).trim();
const payload = [...(await readFile(join(fixture, 'payload.bin')))];
const manifest = (version = '0.2.0', sig = signature) => ({
  version,
  notes: 'Synthetic signed fixture',
  platforms: Object.fromEntries(
    ['windows-x86_64', 'linux-x86_64', 'darwin-x86_64', 'darwin-aarch64'].map((target) => [
      target,
      { signature: sig, url: 'https://example.invalid/payload.exe' },
    ]),
  ),
});
let instance;
try {
  instance = await launch(directory);
  let { main, invoke } = instance;
  const mode = (value) => invoke('update_fixture', { mode: value });
  const status = () => invoke('update_status');
  const verify = (args) =>
    invoke('update_verify_fixture', args).then(
      () => true,
      () => false,
    );
  await until(async () => (await invoke('index_status')).count === 2, 'initial filename catalog');
  assert.equal(
    await verify({ publicKey, manifest: manifest(), payload }),
    true,
    'actual signed-version verifier accepts fixture',
  );
  assert.equal(
    await verify({ publicKey: otherKey, manifest: manifest(), payload }),
    false,
    'different valid key rejected',
  );
  const changed = [...payload];
  changed[0] ^= 1;
  assert.equal(
    await verify({ publicKey, manifest: manifest(), payload: changed }),
    false,
    'tampering rejected',
  );
  assert.equal(
    await verify({ publicKey, manifest: manifest('0.3.0'), payload }),
    false,
    'inflated announced version rejected',
  );
  assert.equal(
    await verify({ publicKey, manifest: manifest('0.2.0', legacy), payload }),
    false,
    'unbound legacy signature rejected',
  );
  const initial = (await invoke('get_settings')).revision;
  await main.getByRole('button', { name: '설정', exact: true }).click();
  let settings = main.getByRole('dialog', { name: '설정', exact: true });
  let panel = settings.getByRole('region', { name: '업데이트', exact: true });
  await mode('ready');
  await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
  await panel.getByText('새 버전이 있습니다 · 0.2.0', { exact: true }).waitFor();
  assert.equal(
    await panel.locator('.update-notes').innerText(),
    'Synthetic release notes <script>plain text only</script>',
  );
  assert.equal(await panel.locator('script').count(), 0);
  assert.equal(
    await panel.getByRole('button', { name: '다운로드 페이지 열기', exact: true }).count(),
    1,
  );
  await panel.screenshot({ path: join(evidence, 'updates-panel.png') });
  await settings.getByRole('button', { name: '취소', exact: true }).click();
  let banner = main.getByRole('status', { name: '업데이트', exact: true });
  await banner.getByRole('button', { name: '나중에', exact: true }).click();
  await until(async () => (await status()).dismissedVersion === '0.2.0', 'dismissal persisted');
  assert.equal(
    (await invoke('get_settings')).revision,
    initial,
    'bookkeeping does not change settings revision',
  );
  assert.equal(
    JSON.parse(await readFile(join(directory, 'updates/state.json'), 'utf8')).dismissedVersion,
    '0.2.0',
  );
  await until(async () => {
    const state = await invoke('content_status');
    return state.phase === 'ready' && state.indexed === 1;
  }, 'update bookkeeping remains outside content indexing');
  const internal = await invoke('search_content', {
    query: 'lastCheckMs',
    extension: '',
    scopeId: 'body',
    requestId: crypto.randomUUID(),
  });
  assert.equal(internal.hits.length, 0, 'update state is never a document source');
  await main.getByRole('button', { name: '설정', exact: true }).click();
  settings = main.getByRole('dialog', { name: '설정', exact: true });
  panel = settings.getByRole('region', { name: '업데이트', exact: true });
  await mode('offline');
  await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
  await panel.getByRole('alert').filter({ hasText: '업데이트를 확인하지 못했습니다' }).waitFor();
  assert.equal((await status()).available.version, '0.2.0', 'offline retains known update');
  assert.equal(await panel.getByText('최신 버전입니다.', { exact: true }).count(), 0);
  await mode('none');
  await invoke('update_check');
  await until(
    async () =>
      (await panel
        .getByRole('alert')
        .filter({ hasText: '업데이트를 확인하지 못했습니다' })
        .count()) === 0,
    'later background success clears old manual failure',
  );
  await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
  await panel.getByText('최신 버전입니다.', { exact: true }).waitFor();
  await mode('offline');
  await invoke('update_check');
  await until(
    async () => (await panel.getByText('최신 버전입니다.', { exact: true }).count()) === 0,
    'failed background check never claims up to date',
  );
  assert.equal(
    await panel.getByRole('alert').filter({ hasText: '업데이트를 확인하지 못했습니다' }).count(),
    0,
    'background failure remains quiet',
  );
  await mode('newer');
  await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
  await panel.getByText('새 버전이 있습니다 · 0.3.0', { exact: true }).waitFor();
  await settings.getByRole('button', { name: '취소', exact: true }).click();
  await banner.getByText('새 버전이 있습니다 · 0.3.0', { exact: true }).waitFor();
  await banner.screenshot({ path: join(evidence, 'updates-banner.png') });
  await banner.getByRole('button', { name: '나중에', exact: true }).click();
  await until(async () => (await status()).dismissedVersion === '0.3.0', 'newer dismissal');
  await main.getByRole('button', { name: '설정', exact: true }).click();
  settings = main.getByRole('dialog', { name: '설정', exact: true });
  panel = settings.getByRole('region', { name: '업데이트', exact: true });
  for (const scenario of ['signatureFailed', 'installFailed']) {
    await mode(scenario);
    await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
    await until(async () => (await status()).canInstall, 'synthetic install branch');
    const indexBefore = await invoke('index_status');
    await panel.getByRole('button', { name: '설치하고 다시 시작', exact: true }).click();
    const confirmation = main.getByRole('dialog', { name: '업데이트를 설치할까요?', exact: true });
    await confirmation.getByRole('button', { name: '설치하고 다시 시작', exact: true }).click();
    await confirmation
      .getByRole('alert')
      .filter({ hasText: '업데이트를 설치하지 못했습니다' })
      .waitFor();
    await until(
      async () => (await invoke('index_status')).count === 2,
      'failed install restores filename scope',
    );
    assert.equal((await invoke('clipboard_status')).paused, false);
    assert.equal((await invoke('get_settings')).revision, initial);
    assert.equal((await mode('snapshot')).installs, 1);
    if (scenario === 'signatureFailed')
      assert.equal(
        (await invoke('index_status')).revision,
        indexBefore.revision,
        'verification failure never prepares/stops workers',
      );
    else await confirmation.screenshot({ path: join(evidence, 'updates-install-failure.png') });
    await confirmation.getByRole('button', { name: '취소', exact: true }).click();
  }
  await mode('installFailed');
  await panel.getByRole('button', { name: '지금 확인', exact: true }).click();
  await until(async () => (await status()).canInstall, 'selected release available');
  await panel.getByRole('button', { name: '설치하고 다시 시작', exact: true }).click();
  const stale = main.getByRole('dialog', { name: '업데이트를 설치할까요?', exact: true });
  await stale.getByText('Prism 0.2.0', { exact: true }).waitFor();
  await mode('newerInstallFailed');
  await invoke('update_check');
  await until(
    async () => (await status()).available.version === '0.3.0',
    'new release arrives while confirming',
  );
  await stale.getByRole('button', { name: '설치하고 다시 시작', exact: true }).click();
  await stale.getByRole('alert').filter({ hasText: '확인하던 업데이트가 바뀌었습니다' }).waitFor();
  assert.equal(
    (await mode('snapshot')).installs,
    0,
    'changed confirmation never enters installer transport',
  );
  assert.equal(
    await stale.getByRole('button', { name: '설치하고 다시 시작', exact: true }).isDisabled(),
    true,
  );
  await stale.getByRole('button', { name: '취소', exact: true }).click();
  await panel.getByLabel('새 버전 자동 확인', { exact: true }).uncheck();
  assert.equal(
    await panel.getByRole('button', { name: '설치하고 다시 시작', exact: true }).isDisabled(),
    true,
    'unsaved preferences block restart',
  );
  await settings.getByRole('button', { name: '저장', exact: true }).click();
  await until(async () => !(await status()).autoCheck, 'saved auto-check preference applied');
  const metadata = JSON.parse(await readFile(join(directory, 'updates/state.json'), 'utf8'));
  assert.equal(metadata.dismissedVersion, '0.3.0');
  assert(metadata.lastCheckMs > 0);
  // Rendering contract only: this isolated process is not a registered MSIX package.
  // Even an inconsistent event must not offer an ordinary installer for a package.
  await main.getByRole('button', { name: '설정', exact: true }).click();
  settings = main.getByRole('dialog', { name: '설정', exact: true });
  panel = settings.getByRole('region', { name: '업데이트', exact: true });
  const realView = await status();
  await invoke('plugin:event|emit', {
    event: 'prism:updates',
    payload: {
      ...realView,
      available: { version: '0.4.0', notes: 'Synthetic MSIX presentation check' },
      dismissedVersion: null,
      canInstall: true,
      installDisabledReason: 'packaged',
    },
  });
  await panel.getByText('MSIX 버전은 Microsoft Store', { exact: false }).waitFor();
  assert.equal(
    await panel.getByRole('button', { name: '설치하고 다시 시작', exact: true }).count(),
    0,
  );
  assert.equal(
    await panel.getByRole('button', { name: '다운로드 페이지 열기', exact: true }).count(),
    0,
  );
  await panel.screenshot({ path: join(evidence, 'updates-msix-policy.png') });
  await settings.getByRole('button', { name: '취소', exact: true }).click();
  const packagedBanner = main.locator('.update-banner');
  await packagedBanner.getByText('MSIX 버전은 Microsoft Store', { exact: false }).waitFor();
  assert.equal(
    await packagedBanner.getByRole('button', { name: '설치하고 다시 시작', exact: true }).count(),
    0,
  );
  assert.equal(
    await packagedBanner.getByRole('button', { name: '다운로드 페이지 열기', exact: true }).count(),
    0,
  );
  await invoke('plugin:event|emit', { event: 'prism:updates', payload: realView });
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;
  instance = await launch(directory);
  ({ main, invoke } = instance);
  assert.equal((await status()).autoCheck, false);
  assert.equal((await status()).dismissedVersion, '0.3.0');
  assert.equal((await status()).lastCheckMs, metadata.lastCheckMs);
  assert.equal(
    (await status()).available,
    null,
    'unverified remote releases are not restored from disk',
  );
  await until(async () => {
    const state = await invoke('content_status');
    return state.phase === 'ready' && state.indexed === 1;
  }, 'startup excludes existing update metadata');
  assert.deepEqual(instance.errors, []);
  await shutdown(instance);
  instance = null;
  const corruptDirectory = await mkdtemp(join(tmpdir(), 'prism-updates-corrupt-'));
  await writeFile(join(corruptDirectory, 'settings.json'), 'broken settings must survive');
  instance = await launch(corruptDirectory, { readyHeading: false });
  ({ main, invoke } = instance);
  await main.locator('.load-state h1').waitFor();
  await mode('installFailed');
  await invoke('update_check');
  const corruptFailure = await invoke('update_install', { version: '0.2.0' }).then(
    () => '',
    (cause) => String(cause),
  );
  assert.match(corruptFailure, /Repair application settings before updating/);
  assert.equal(
    await readFile(join(corruptDirectory, 'settings.json'), 'utf8'),
    'broken settings must survive',
    'preparation never replaces a corrupt profile with defaults',
  );
  assert.equal((await status()).installing, false);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      verification: 'actual Tauri signed-version verifier',
      transport: 'synthetic',
      downloads: 0,
      installerExecutions: 0,
    }),
  );
} finally {
  if (instance) await shutdown(instance);
}
