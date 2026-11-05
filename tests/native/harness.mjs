// Only launches the test's own process with isolated settings, WebView and clipboard.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
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
export async function launch(dataDir, options = {}) {
  const reservation = createServer();
  await new Promise((done) => reservation.listen(0, '127.0.0.1', done));
  const port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  const environment = {
    ...process.env,
    PRISM_DEV_DATA_DIR: dataDir,
    WEBVIEW2_USER_DATA_FOLDER: join(dataDir, 'webview'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  };
  if (options.portable) {
    delete environment.PRISM_DEV_DATA_DIR;
    delete environment.WEBVIEW2_USER_DATA_FOLDER;
    environment.PRISM_DEV_TEST_MODE = '1';
  }
  const child = spawn(resolve(options.executable ?? 'src-tauri/target/debug/prism.exe'), [], {
    windowsHide: true,
    stdio: 'ignore',
    env: environment,
  });
  let browser;
  try {
    await until(async () => {
      if (child.exitCode !== null) throw new Error(`Prism exited: ${child.exitCode}`);
      try {
        return (await fetch(`http://127.0.0.1:${port}/json/version`)).ok;
      } catch {
        return false;
      }
    }, 'isolated WebView endpoint');
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    let main, quick;
    await until(() => {
      const pages = browser.contexts().flatMap((context) => context.pages());
      main = pages.find(
        (page) => page.url().includes('tauri.localhost') && !page.url().includes('#quick'),
      );
      quick = pages.find(
        (page) => page.url().includes('tauri.localhost') && page.url().includes('#quick'),
      );
      return main && quick;
    }, 'bundled main and Spotlight windows');
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
    return { child, browser, main, quick, errors, invoke };
  } catch (error) {
    await browser?.close();
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
  await instance.browser.close();
  await until(() => instance.child.exitCode !== null, 'owned test process exit').catch(() =>
    instance.child.kill(),
  );
}
