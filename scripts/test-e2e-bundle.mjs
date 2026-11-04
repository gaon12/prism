// Run the unchanged browser assertions against already-built production assets.
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
const child = spawn(
  process.execPath,
  [resolve('node_modules/@playwright/test/cli.js'), 'test', ...process.argv.slice(2)],
  {
    env: { ...process.env, PRISM_E2E_PREVIEW: '1' },
    stdio: 'inherit',
    windowsHide: true,
  },
);
child.on('error', () => {
  console.error('Could not start bundled browser tests.');
  process.exitCode = 1;
});
child.on('exit', (code) => {
  process.exitCode = code ?? 1;
});
