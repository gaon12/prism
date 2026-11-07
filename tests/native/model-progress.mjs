// Actual pinned model download and filename watcher, in an isolated profile.
// Browser plugin not available: inspect the bundled native WebView over CDP.
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, mkdtemp, stat, writeFile } from 'node:fs/promises';
import { freemem, tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

assert.equal(process.platform, 'win32', 'This WebView2 regression requires Windows');
const expectedRelease = process.env.PRISM_TEST_EXPECT_RELEASE ?? '0';
assert(['0', '1'].includes(expectedRelease), 'PRISM_TEST_EXPECT_RELEASE must be 0 or 1');
const candidate = resolve(process.env.PRISM_TEST_EXECUTABLE ?? 'src-tauri/target/debug/prism.exe');
assert.equal(basename(candidate).toLowerCase(), 'prism.exe');
assert((await stat(candidate)).isFile());
async function sha(path) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest('hex');
}
const candidateSha256 = await sha(candidate);
const directory = await mkdtemp(join(tmpdir(), 'prism-model-progress-'));
await writeFile(join(directory, 'qa-owner.txt'), 'Prism real download regression v1\n');
// A marker beside our COPY forces both release and debug candidates into the
// owned profile even if the caller supplied the wrong expected-build flag.
const application = join(directory, 'app');
const profile = join(application, 'PrismData');
const evidence = join(directory, 'evidence');
await Promise.all([profile, evidence].map((path) => mkdir(path, { recursive: true })));
const executable = join(application, 'prism.exe');
await copyFile(candidate, executable);
await writeFile(join(application, 'prism.portable'), '');
assert.equal(await sha(executable), candidateSha256);
const files = join(directory, 'files');
await mkdir(files);
const scopeId = `model_progress_${randomUUID()}`;
const scopePath = process.env.PRISM_PROGRESS_SCOPE ?? files;
await writeFile(
  join(profile, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    ocr: { enabled: false },
    automaticSemantic: false,
    hybridSearch: false,
    generation: { mode: 'local', fallback: false, allowExternalQuestions: false },
    scopes: [
      {
        id: scopeId,
        name: 'Files',
        path: scopePath,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
let instance;
let child;
let verifiedProfile = false;
let writes;
let writer = Promise.resolve();
let writerError;
let guard;
let minimumAvailable = freemem();
let resourceStop = false;
const warnings = [];
const result = {
  passed: false,
  directory,
  evidence,
  candidateSha256,
  expectedRelease: expectedRelease === '1',
  responsesReplaced: false,
  inferenceRequested: false,
  externalScopeOverride: Boolean(process.env.PRISM_PROGRESS_SCOPE),
};
const check = () => {
  assert(!resourceStop, 'Owned app stopped below1GiB available memory');
  assert(!writerError, String(writerError));
  if (child) assert(child.exitCode === null && child.signalCode === null, 'Owned app exited');
};
try {
  result.availableBefore = freemem();
  assert(
    result.availableBefore >= 3 * 1024 ** 3,
    'Need3GiB available before the real download UI check',
  );
  minimumAvailable = result.availableBefore;
  guard = setInterval(() => {
    minimumAvailable = Math.min(minimumAvailable, freemem());
    if (minimumAvailable < 1024 ** 3 && !resourceStop) {
      resourceStop = true;
      if (child?.exitCode === null && child?.signalCode === null) child.kill();
    }
  }, 250);
  instance = await launch(profile, {
    executable,
    portable: true,
    productionPortable: expectedRelease === '1',
    cdpPort: process.env.PRISM_APP_CDP_PORT ? Number(process.env.PRISM_APP_CDP_PORT) : undefined,
    onSpawn: (owned) => {
      child = owned;
    },
  });
  const { main, invoke } = instance;
  for (const page of [main, instance.quick])
    page.on('console', (entry) => {
      if (entry.type() === 'warning') warnings.push(entry.text());
    });
  const settings = await invoke('get_settings');
  assert.equal(settings.scopes.length, 1);
  assert(settings.scopes[0].id === scopeId, 'CDP must reach this fixture profile');
  assert(
    resolve(settings.scopes[0].path).toLowerCase() === resolve(scopePath).toLowerCase(),
    'Unexpected fixture scope path',
  );
  verifiedProfile = true;
  result.runtime = await invoke('runtime_info');
  assert.equal(result.runtime.installKind, 'portable');
  assert.equal(result.runtime.development, expectedRelease !== '1');
  assert.equal(result.runtime.startupError, null);
  assert(
    !settings.clipboard.enabled &&
      !settings.ocr.enabled &&
      !settings.updates.autoCheck &&
      !settings.startOnLogin,
  );
  assert(!settings.scopes[0].contentEnabled && !settings.scopes[0].externalAllowed);
  assert.equal((await invoke('model_status')).pid, null);
  await main.getByRole('button', { name: 'AI·모델 설정 열기', exact: true }).click();
  await until(
    async () => (await invoke('model_status')).phase === 'notInstalled',
    'isolated model absent',
  );
  // A real download is required here; no model or progress DTO is substituted.
  await main.locator('.embedding-install button').click();
  await until(
    async () => {
      check();
      const status = await invoke('model_status');
      assert.equal(status.pid, null, 'Downloading must not start inference');
      assert.notEqual(status.phase, 'error', status.lastError ?? 'download failed');
      return status.downloadedBytes > 1024 * 1024;
    },
    'official model download begins',
    60_000,
  );
  await main.locator('.settings-body').evaluate((body) => {
    body.scrollTop = 0;
  });
  const phase = await invoke('model_status');
  const initialIndex = await invoke('index_status');
  let nextFile = 0;
  writes = setInterval(() => {
    // Serialize fixture writes so cleanup also waits for the last one.
    writer = writer
      .then(() =>
        writerError
          ? undefined
          : writeFile(join(files, `public-${++nextFile}.txt`), 'public watcher fixture'),
      )
      .catch((error) => {
        writerError = error;
      });
  }, 100);
  await main.evaluate(() => {
    const dialog = document.querySelector('.settings-modal');
    const progress = document.querySelector('.embedding-progress');
    const samples = [];
    window.__modelProgressSamples = samples;
    window.__stopProgressObservation = false;
    const frame = () => {
      const rect = dialog.getBoundingClientRect();
      samples.push({
        same: dialog === document.querySelector('.settings-modal'),
        sameProgress: progress === document.querySelector('.embedding-progress'),
        open: dialog.open,
        opacity: getComputedStyle(dialog).opacity,
        rect: [rect.x, rect.y, rect.width, rect.height],
        animations: dialog.getAnimations().filter((animation) => animation.playState === 'running')
          .length,
      });
      if (!window.__stopProgressObservation) requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  });
  await until(async () => {
    check();
    const current = await invoke('model_status');
    assert.equal(current.pid, null);
    assert.equal(current.phase, 'downloading', 'Real transfer ended before sustained observation');
    result.progress = current;
    return current.downloadedBytes > phase.downloadedBytes + 2 * 1024 * 1024;
  }, 'real byte count advances');
  await new Promise((done) => setTimeout(done, 4000));
  check();
  assert.equal(
    (await invoke('model_status')).phase,
    'downloading',
    'Transfer completed before cancellation; this case is not qualified',
  );
  await main.locator('.settings-body').evaluate((body) => {
    body.scrollTop = 0;
  });
  const geometry = await main.locator('.embedding-progress').evaluate((panel) => {
    const bytes = panel.querySelector('small').getBoundingClientRect();
    const button = panel.querySelector('button').getBoundingClientRect();
    return { bytesBottom: bytes.bottom, buttonTop: button.top, buttonBottom: button.bottom };
  });
  assert(geometry.buttonTop >= geometry.bytesBottom + 6, 'cancel has its own row below capacity');
  const samples = await main.evaluate(() => {
    window.__stopProgressObservation = true;
    return window.__modelProgressSamples;
  });
  assert(samples.length > 30);
  assert(
    samples.every(
      (sample) =>
        sample.same &&
        sample.sameProgress &&
        sample.open &&
        sample.opacity === '1' &&
        sample.animations === 0,
    ),
    'download and index changes must not reopen or animate settings',
  );
  assert.equal(
    new Set(samples.map((sample) => JSON.stringify(sample.rect))).size,
    1,
    'dialog geometry stays stable',
  );
  assert.equal(
    await main
      .locator('.settings-modal')
      .evaluate((dialog) => getComputedStyle(dialog, '::backdrop').backdropFilter),
    'none',
  );
  await until(
    async () => (await invoke('index_status')).revision > initialIndex.revision,
    'actual filename index changes behind settings',
  );
  result.frames = samples.length;
  result.geometry = geometry;
  await main.screenshot({ path: join(evidence, 'model-progress-desktop.png') });
  await main.setViewportSize({ width: 390, height: 800 });
  await main.locator('.embedding-progress-actions button').scrollIntoViewIfNeeded();
  assert(
    await main
      .locator('.settings-modal')
      .evaluate((dialog) => dialog.scrollWidth <= dialog.clientWidth),
  );
  await main.screenshot({ path: join(evidence, 'model-progress-narrow.png') });
  assert(await main.locator('.embedding-progress-actions button').isVisible());
  await main.locator('.embedding-progress-actions button').click();
  await until(async () => {
    check();
    result.afterCancel = await invoke('model_status');
    return result.afterCancel.phase === 'cancelled';
  }, 'real model download cancelled');
  assert(await main.locator('.settings-modal').isVisible());
  assert.deepEqual(instance.errors, []);
  const index = await invoke('index_status');
  result.index = {
    count: index.count,
    revision: index.revision,
    skipped: index.skipped,
    warningCount: index.warnings.length,
  };
  assert.equal(result.afterCancel.pid, null);
  assert.equal((await invoke('content_status')).indexed, 0);
  result.passed = true;
} catch (error) {
  result.error = verifiedProfile
    ? String(error.stack ?? error)
    : 'Portable fixture profile was not verified; no download or cleanup IPC was sent';
  if (instance && verifiedProfile)
    await instance.main.screenshot({ path: join(evidence, 'failure.png') }).catch(() => {});
  throw verifiedProfile ? error : new Error(result.error);
} finally {
  clearInterval(writes);
  await writer;
  if (instance && verifiedProfile) {
    await instance.main
      .evaluate(() => {
        window.__stopProgressObservation = true;
      })
      .catch(() => {});
    await instance.invoke('model_cancel').catch(() => {});
    await shutdown(instance).catch((error) => {
      result.shutdownError = String(error);
    });
  } else if (instance) {
    // Detach from an unverified endpoint. Only the child handle created above
    // may be terminated; never send stop/quit to an unrelated application.
    await instance.browser.close().catch(() => {});
  }
  if (child && child.exitCode === null && child.signalCode === null) {
    child.kill();
    await until(
      () => child.exitCode !== null || child.signalCode !== null,
      'owned app cleanup',
      5000,
    ).catch((error) => {
      result.shutdownError = String(error);
    });
  }
  clearInterval(guard);
  result.minimumAvailable = minimumAvailable;
  result.resourceStop = resourceStop;
  result.profileVerified = verifiedProfile;
  result.errors = verifiedProfile ? (instance?.errors ?? []) : [];
  result.warnings = verifiedProfile ? warnings : [];
  result.appExitCode = child?.exitCode ?? null;
  result.appExitSignal = child?.signalCode ?? null;
  result.appUnchanged = (await sha(executable).catch(() => null)) === candidateSha256;
  result.candidateUnchanged = (await sha(candidate).catch(() => null)) === candidateSha256;
  let finalFailure;
  if (
    result.passed &&
    (result.shutdownError ||
      writerError ||
      resourceStop ||
      result.appExitCode !== 0 ||
      !result.appUnchanged ||
      !result.candidateUnchanged)
  ) {
    result.passed = false;
    finalFailure = new Error('Download UI checks completed but fixture cleanup/integrity failed');
    result.error = String(finalFailure);
  }
  await writeFile(join(evidence, 'result.json'), JSON.stringify(result, null, 2));
  console.log(
    JSON.stringify({
      passed: result.passed,
      evidence,
      frames: result.frames,
      resourceStop,
      availableBefore: result.availableBefore,
      minimumAvailable,
      error: result.error,
    }),
  );
  if (finalFailure) throw finalFailure;
}
