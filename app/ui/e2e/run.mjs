import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const uiDir = fileURLToPath(new URL('..', import.meta.url));

const env = {
  ...process.env,
  PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS: '1',
  PW_TEST_HTML_REPORT_OPEN: 'never',
};

const result = spawnSync(
  process.execPath,
  [join(uiDir, 'node_modules', '@playwright', 'test', 'cli.js'), 'test', '--config', 'e2e/playwright.config.ts', '--workers=1', ...process.argv.slice(2)],
  { cwd: uiDir, env, stdio: 'inherit' },
);
process.exit(result.status ?? 1);
