import { defineConfig } from '@playwright/test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const bundled = process.env.PRISM_E2E_PREVIEW === '1';

export default defineConfig({
  testDir: './tests/e2e',
  outputDir: join(tmpdir(), bundled ? 'prism-bundle-e2e-results' : 'prism-e2e-results'),
  fullyParallel: true,
  workers: 2,
  timeout: 30_000,
  use: {
    baseURL: bundled ? 'http://127.0.0.1:1421' : 'http://127.0.0.1:1420',
    viewport: { width: 1280, height: 840 },
    colorScheme: 'light',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: bundled ? 'npm run preview -- --port 1421 --strictPort' : 'npm run dev',
    url: bundled ? 'http://127.0.0.1:1421' : 'http://127.0.0.1:1420',
    reuseExistingServer: !bundled && !process.env.CI,
  },
});
