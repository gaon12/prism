// Only launches the test's own process with isolated settings, WebView and clipboard.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
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
export async function launch(dataDir) {
  const reservation = createServer();
  await new Promise((done) => reservation.listen(0, '127.0.0.1', done));
  const port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  const child = spawn(resolve('src-tauri/target/debug/prism.exe'), [], {
    windowsHide: true,
    stdio: 'ignore',
    env: {
      ...process.env,
      PRISM_DEV_DATA_DIR: dataDir,
      WEBVIEW2_USER_DATA_FOLDER: join(dataDir, 'webview'),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
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
    await main.getByRole('heading', { name: '파일 검색', exact: true }).waitFor();
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
