import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { chromium } from '@playwright/test';

// Run against a development binary and a running Vite server. All writes stay in this test directory.
const dataDir = await mkdtemp(join(tmpdir(), 'prism-native-'));
const evidence = join(tmpdir(), 'prism-ui-evidence');
await mkdir(evidence, { recursive: true });
const reservation = createServer();
await new Promise((done) => reservation.listen(0, '127.0.0.1', done));
const port = reservation.address().port;
await new Promise((done) => reservation.close(done));
const executable = resolve('src-tauri/target/debug/prism.exe');
let child;
let browser;
let stderr = '';
const errors = [];
async function until(check, label) {
  const deadline = Date.now() + 25_000;
  while (Date.now() < deadline) {
    if (child?.exitCode != null) throw new Error(`Prism exited: ${stderr}`);
    try {
      const value = await check();
      if (value) return value;
    } catch {
      /* Native WebView may still be starting. */
    }
    await new Promise((done) => setTimeout(done, 150));
  }
  throw new Error(`Timed out: ${label}\n${stderr}`);
}
async function start(expectSettingsError = false) {
  child = spawn(executable, [], {
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: {
      ...process.env,
      PRISM_DEV_DATA_DIR: dataDir,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  await until(
    async () => (await fetch(`http://127.0.0.1:${port}/json/version`)).ok,
    'WebView2 debugging endpoint',
  );
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const pages = await until(() => {
    const all = browser.contexts().flatMap((context) => context.pages());
    return all.filter((page) => page.url().includes('127.0.0.1:1420')).length >= 2 ? all : null;
  }, 'two native windows');
  for (const page of pages) page.on('pageerror', (error) => errors.push(error.message));
  const main = pages.find((page) => !page.url().includes('#quick'));
  const quick = pages.find((page) => page.url().includes('#quick'));
  assert.ok(main && quick);
  await main
    .getByRole('heading', {
      name: expectSettingsError ? '설정을 불러오지 못했어요' : '파일 검색',
      exact: true,
    })
    .waitFor();
  return { main, quick };
}
async function invoke(page, command, args = {}) {
  return page.evaluate(
    async ({ command, args }) => {
      const { invoke } = await import('/node_modules/@tauri-apps/api/core.js');
      return invoke(command, args);
    },
    { command, args },
  );
}
async function visible(page) {
  return page.evaluate(async () => {
    const { getCurrentWindow } = await import('/node_modules/@tauri-apps/api/window.js');
    return getCurrentWindow().isVisible();
  });
}
try {
  let { main, quick } = await start();
  assert.equal(await visible(main), true);
  assert.equal(await visible(quick), false);
  const initial = await invoke(main, 'get_settings');
  assert.equal(initial.language, 'ko');
  assert.equal((await invoke(main, 'runtime_info')).native, true);
  await main.getByRole('textbox', { name: '검색어', exact: true }).fill('한국어 네이티브 검색');
  await main.getByRole('button', { name: '빠른 검색', exact: true }).click();
  await until(() => visible(quick), 'quick window visible');
  assert.equal(
    await quick.getByRole('textbox', { name: '검색어', exact: true }).inputValue(),
    '한국어 네이티브 검색',
  );
  await quick.getByRole('button', { name: '일반 창으로 열기', exact: true }).click();
  await until(async () => !(await visible(quick)), 'quick window hidden after handoff');
  assert.equal(
    await main.getByRole('textbox', { name: '검색어', exact: true }).inputValue(),
    '한국어 네이티브 검색',
  );

  let saved;
  for (const shortcut of ['Ctrl+Alt+Shift+F11', 'Ctrl+Alt+Shift+F10', 'Ctrl+Alt+Shift+F9']) {
    try {
      saved = await invoke(main, 'save_settings', {
        settings: {
          ...initial,
          shortcut,
          scopes: [
            {
              id: 'native-test',
              name: '네이티브 테스트',
              path: dataDir,
              filenameEnabled: true,
              contentEnabled: false,
            },
          ],
        },
        expectedRevision: initial.revision,
      });
      break;
    } catch (error) {
      if (!String(error).includes('Shortcut unavailable')) throw error;
    }
  }
  assert.ok(saved, 'a global shortcut must register');
  assert.equal((await invoke(main, 'runtime_info')).shortcutError, null);
  await assert.rejects(
    invoke(main, 'save_settings', {
      settings: { ...saved, shortcut: 'A' },
      expectedRevision: saved.revision,
    }),
    /modifier/,
  );
  await assert.rejects(
    invoke(main, 'save_settings', { settings: saved, expectedRevision: initial.revision }),
    /another window/,
  );
  const persisted = JSON.parse(await readFile(join(dataDir, 'settings.json'), 'utf8'));
  assert.equal(persisted.shortcut, saved.shortcut);
  assert.equal(persisted.scopes[0].contentEnabled, false);
  assert.equal(persisted.scopes[0].filenameEnabled, true);
  await main.getByRole('heading', { name: '검색 범위가 준비됐어요' }).waitFor();
  await main.screenshot({ path: join(evidence, 'native-main.png') });
  const markdownPath = join(dataDir, '네이티브 문서.md');
  await invoke(main, 'write_markdown', {
    path: markdownPath,
    contents: '# 네이티브 저장\n\nPrism 테스트',
  });
  assert.match(await readFile(markdownPath, 'utf8'), /네이티브 저장/);
  const exited = once(child, 'exit');
  await invoke(main, 'quit_app').catch(() => undefined);
  await exited;
  browser = null;
  ({ main, quick } = await start());
  const restored = await invoke(main, 'get_settings');
  assert.equal(restored.revision, saved.revision);
  assert.equal(restored.scopes[0].name, '네이티브 테스트');
  assert.equal(
    await main.getByRole('textbox', { name: '검색어', exact: true }).inputValue(),
    '한국어 네이티브 검색',
  );
  await invoke(main, 'show_quick');
  await until(() => visible(quick), 'restored quick window');
  await quick.screenshot({ path: join(evidence, 'native-quick.png') });
  const exitedAgain = once(child, 'exit');
  await invoke(main, 'quit_app').catch(() => undefined);
  await exitedAgain;
  browser = null;
  const settingsText = await readFile(join(dataDir, 'settings.json'), 'utf8');
  await writeFile(join(dataDir, 'settings.json'), '{broken');
  ({ main } = await start(true));
  assert.equal(await readFile(join(dataDir, 'settings.json'), 'utf8'), '{broken');
  await writeFile(join(dataDir, 'settings.json'), settingsText);
  await main.getByRole('button', { name: '다시 시도', exact: true }).click();
  await main.getByRole('heading', { name: '검색 범위가 준비됐어요' }).waitFor();
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify(
      {
        result: 'passed',
        dataDir,
        evidence,
        verified: [
          'two native windows',
          'query handoff',
          'global shortcut registration',
          'invalid shortcut rejection',
          'stale revision rejection',
          'native settings persistence',
          'independent scopes',
          'Markdown write',
          'restart restoration',
          'corrupt settings recovery without data replacement',
        ],
      },
      null,
      2,
    ),
  );
} finally {
  if (browser) {
    const main = browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((page) => !page.url().includes('#quick'));
    if (main) {
      try {
        await invoke(main, 'quit_app');
      } catch {
        /* The process may already have exited. */
      }
    }
  }
  if (child && child.exitCode == null) {
    const stopped = once(child, 'exit');
    child.kill();
    await stopped;
  }
}
