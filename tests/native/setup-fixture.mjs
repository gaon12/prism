import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

// Build the production installer core without the business app or its Lance dependencies.
export function recoveryFixture() {
  if (process.env.PRISM_RECOVERY_FIXTURE) return process.env.PRISM_RECOVERY_FIXTURE;
  const built = spawnSync(
    'cargo',
    [
      'test',
      '--manifest-path',
      'installer/core/Cargo.toml',
      '--no-run',
      '--message-format=json',
      '--locked',
    ],
    {
      env: {
        ...process.env,
        CARGO_TARGET_DIR: process.env.PRISM_SETUP_CORE_TARGET ?? resolve('installer/core/target'),
      },
      encoding: 'utf8',
      windowsHide: true,
      timeout: 180000,
    },
  );
  assert.equal(built.status, 0, 'Could not build the isolated recovery fixture.');
  const artifacts = built.stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter((item) => item.reason === 'compiler-artifact' && item.profile?.test && item.executable);
  assert.equal(artifacts.length, 1, 'Expected one installer/core test executable.');
  return artifacts[0].executable;
}
