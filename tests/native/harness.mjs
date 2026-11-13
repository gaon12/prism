// Only launches the test's own process with isolated settings, WebView and clipboard.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { lstat } from 'node:fs/promises';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';
import { chromium } from '@playwright/test';

export async function until(check, label, timeout = 25_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`Timed out: ${label}`);
}

// Pick the WebView debug port for a setup test.
// Normally any free port works, passed through WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS.
// WebView2 150+ ignores that variable when the app runs elevated, and CI runners run as admin.
// There CI writes a machine policy with a fixed port for prism-setup.exe and tells us the port
// through PRISM_SETUP_CDP_PORT (see .github/workflows/release.yml).
export async function setupDebugPort() {
  const fixed = Number(process.env.PRISM_SETUP_CDP_PORT);
  if (Number.isInteger(fixed) && fixed > 1024 && fixed < 65536) return fixed;
  const server = createServer();
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

// The very first WebView2 start on a fresh CI machine can take much longer than on a warm PC.
export const WEBVIEW_START_TIMEOUT = 90_000;

// Wait for a launched app's WebView debug endpoint. If it never appears, add a short note on
// what is running (only process names and counts, never window titles or user content), so a
// CI log can tell "slow start" apart from "no WebView was created at all".
// `stillOk` may throw to stop early, e.g. when the process could not be started at all.
export async function untilWebView(child, port, label, stillOk = () => {}) {
  try {
    await until(
      async () => {
        stillOk();
        if (child.exitCode !== null) throw new Error(`Setup exited: ${child.exitCode}`);
        try {
          return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
        } catch {
          return false;
        }
      },
      label,
      WEBVIEW_START_TIMEOUT,
    );
  } catch (error) {
    if (process.platform !== 'win32' || !String(error.message).startsWith('Timed out')) throw error;
    const count = (name) =>
      (
        spawnSync('tasklist', ['/FO', 'CSV', '/NH', '/FI', `IMAGENAME eq ${name}`], {
          encoding: 'utf8',
          windowsHide: true,
        }).stdout ?? ''
      )
        .split(/\r?\n/)
        .filter((line) => line.startsWith('"')).length;
    error.message +=
      ` (setup pid ${child.pid} still running; msedgewebview2.exe processes: ` +
      `${count('msedgewebview2.exe')}; WerFault.exe: ${count('WerFault.exe')})`;
    throw error;
  }
}
// Validate before spawning: a missing marker would make a release app use the
// installed profile and could apply its login/clipboard settings. Callers still
// own the executable and fixture directory; this is a preflight, not a sandbox.
export async function assertPortableProfile(dataDir, executable) {
  const info = async (path) => {
    try {
      return await lstat(path);
    } catch (error) {
      if (error.code === 'ENOENT') return null;
      throw new Error('Portable fixture metadata could not be inspected');
    }
  };
  const application = dirname(resolve(executable));
  assert(
    (await info(join(application, 'prism.portable')))?.isFile(),
    'Portable marker must be a regular file beside the executable',
  );
  const expected = join(application, 'PrismData');
  const compare = (path) => (process.platform === 'win32' ? path.toLowerCase() : path);
  assert.equal(
    compare(resolve(dataDir)),
    compare(expected),
    'Portable data directory must be PrismData beside the executable',
  );
  const profile = await info(expected);
  assert(!profile || profile.isDirectory(), 'Portable data directory must not be a file or link');
  assert((await info(resolve(executable)))?.isFile(), 'Portable executable must be a regular file');
}

export async function launch(dataDir, options = {}) {
  assert(
    !options.productionPortable || options.portable,
    'Production portable mode needs portable',
  );
  if (options.portable)
    await assertPortableProfile(dataDir, options.executable ?? 'src-tauri/target/debug/prism.exe');
  let port = options.cdpPort;
  if (port !== undefined) {
    assert(Number.isInteger(port) && port > 1024 && port < 65536, 'Invalid explicit test CDP port');
  }
  // An occupied explicit port could otherwise attach a stale/foreign WebView.
  const reservation = createServer();
  await new Promise((done, reject) => {
    reservation.once('error', reject);
    reservation.listen(port ?? 0, '127.0.0.1', done);
  });
  port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  const environment = {
    ...process.env,
    PRISM_DEV_DATA_DIR: dataDir,
    WEBVIEW2_USER_DATA_FOLDER: join(dataDir, 'webview'),
    // This variable can take the place of the arguments the app passes, so a
    // forced software-drawing run (PRISM_WEBVIEW_SOFTWARE=1, see windows.rs)
    // repeats the app's own --disable-gpu here.
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1${process.env.PRISM_WEBVIEW_SOFTWARE === '1' ? ' --disable-gpu' : ''}`,
  };
  if (options.portable) {
    delete environment.PRISM_DEV_DATA_DIR;
    delete environment.WEBVIEW2_USER_DATA_FOLDER;
    if (options.productionPortable) {
      for (const key of Object.keys(environment))
        if (key.startsWith('PRISM_DEV_')) delete environment[key];
    } else {
      environment.PRISM_DEV_TEST_MODE = '1';
    }
  }
  const child = spawn(resolve(options.executable ?? 'src-tauri/target/debug/prism.exe'), [], {
    windowsHide: true,
    stdio: 'ignore',
    env: environment,
  });
  let spawnError;
  child.on('error', (error) => {
    spawnError = error;
  });
  let browser;
  try {
    // A resource guard must own the process handle even before WebView starts.
    options.onSpawn?.(child);
    // Optional: a caller can see when each startup step finished
    // (harsh-profile.mjs records them to split a slow window).
    const mark = (name) => options.onStep?.(name);
    await until(async () => {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null || child.signalCode !== null)
        throw new Error(`Prism exited: ${child.exitCode ?? child.signalCode}`);
      try {
        return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
      } catch {
        return false;
      }
    }, 'isolated WebView endpoint');
    mark('webview');
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    let main,
      quick,
      seenPage = false;
    await until(
      () => {
        const pages = browser.contexts().flatMap((context) => context.pages());
        // First moment any page exists, and its address then (startup timing only).
        if (pages.length && !seenPage) {
          seenPage = true;
          mark(`page:${pages.map((page) => page.url().slice(0, 40)).join(',')}`);
        }
        main = pages.find(
          (page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'),
        );
        quick = pages.find(
          (page) => page.url().includes('tauri.localhost') && page.url().includes('#quick'),
        );
        return main && quick;
        // A slow-start measurement (harsh-profile.mjs) passes a longer
        // windowTimeout so a slow window is recorded instead of failing the run.
      },
      'bundled main and Spotlight windows',
      options.windowTimeout,
    );
    mark('pages');
    if (options.readyHeading !== false)
      await main
        .getByRole('heading', { name: options.readyHeading ?? '파일 검색', exact: true })
        .waitFor();
    assert.equal(await main.title(), 'Prism');
    const errors = [];
    for (const page of [main, quick]) {
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (message.type() === 'error') errors.push(message.text());
      });
    }
    const invoke = (command, args = {}) =>
      main.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
        command,
        args,
      });
    return { child, browser, main, quick, errors, invoke, port };
  } catch (error) {
    await browser?.close().catch(() => {});
    child.kill();
    throw error;
  }
}
export async function shutdown(instance) {
  try {
    await instance.invoke('model_stop');
  } catch {
    /* no running test model */
  }
  try {
    await instance.invoke('quit_app');
  } catch {
    /* window closes during IPC */
  }
  await instance.browser.close().catch(() => {});
  await until(
    () => instance.child.exitCode !== null || instance.child.signalCode !== null,
    'owned test process exit',
  ).catch(() => instance.child.kill());
}
