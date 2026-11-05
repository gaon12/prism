// Actual pinned model download and filename watcher, in an isolated profile.
// Browser plugin not available: inspect the bundled native WebView over CDP.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { launch, shutdown, until } from './harness.mjs';

const directory = await mkdtemp(join(tmpdir(), 'prism-model-progress-'));
const files = join(directory, 'files');
await mkdir(files);
await writeFile(
  join(directory, 'settings.json'),
  JSON.stringify({
    onboardingVersion: 1,
    startOnLogin: false,
    updates: { autoCheck: false },
    clipboard: { enabled: false, onboarded: true },
    scopes: [
      {
        id: 'files',
        name: 'Files',
        path: process.env.PRISM_PROGRESS_SCOPE ?? files,
        filenameEnabled: true,
        contentEnabled: false,
      },
    ],
  }),
);
let instance;
let writes;
let writer = Promise.resolve();
try {
  instance = await launch(
    directory,
    process.env.PRISM_TEST_EXECUTABLE ? { executable: process.env.PRISM_TEST_EXECUTABLE } : {},
  );
  const { main, invoke } = instance;
  await main.getByRole('button', { name: 'AI·모델 설정 열기', exact: true }).click();
  await until(
    async () => (await invoke('model_status')).phase === 'notInstalled',
    'isolated model absent',
  );
  // A real download is required here; no model or progress DTO is substituted.
  await main.locator('.embedding-install button').click();
  await until(
    async () => {
      const status = await invoke('model_status');
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
    writer = writer.then(() =>
      writeFile(join(files, `public-${++nextFile}.txt`), 'public watcher fixture'),
    );
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
  await until(
    async () =>
      (await invoke('model_status')).downloadedBytes > phase.downloadedBytes + 2 * 1024 * 1024,
    'real byte count advances',
  );
  await new Promise((done) => setTimeout(done, 4000));
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
  await main.screenshot({ path: join(directory, 'model-progress-desktop.png') });
  await main.setViewportSize({ width: 390, height: 800 });
  await main.locator('.embedding-progress-actions button').scrollIntoViewIfNeeded();
  assert(
    await main
      .locator('.settings-modal')
      .evaluate((dialog) => dialog.scrollWidth <= dialog.clientWidth),
  );
  await main.screenshot({ path: join(directory, 'model-progress-narrow.png') });
  assert(await main.locator('.embedding-progress-actions button').isVisible());
  await main.locator('.embedding-progress-actions button').click();
  await until(
    async () => (await invoke('model_status')).phase === 'cancelled',
    'real model download cancelled',
  );
  assert(await main.locator('.settings-modal').isVisible());
  assert.deepEqual(instance.errors, []);
  console.log(
    JSON.stringify({
      passed: true,
      directory,
      frames: samples.length,
      geometry,
      index: await invoke('index_status'),
    }),
  );
} finally {
  clearInterval(writes);
  await writer;
  if (instance) {
    await instance.invoke('model_cancel').catch(() => {});
    await shutdown(instance);
  }
}
