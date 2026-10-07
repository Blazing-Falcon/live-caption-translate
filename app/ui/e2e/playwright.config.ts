import { fileURLToPath } from 'node:url';
import { defineConfig } from '@playwright/test';

const here = (relative: string): string => fileURLToPath(new URL(relative, import.meta.url));

export default defineConfig({
  testDir: here('./specs'),
  outputDir: here('../test-results/playwright'),
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 60_000,
  reporter: [['list']],
  use: {
    headless: true,
    baseURL: 'http://127.0.0.1:1431',
    viewport: { width: 1280, height: 720 },
    locale: 'en-US',
    colorScheme: 'light',
  },
  webServer: {
    command: 'npx vite --config vite.e2e.config.ts --host 127.0.0.1 --port 1431 --strictPort',
    cwd: here('..'),
    url: 'http://127.0.0.1:1431/e2e/harness/control.html',
    reuseExistingServer: false,
    timeout: 60_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
