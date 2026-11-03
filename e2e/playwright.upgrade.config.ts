// The v1 -> v2 upgrade with real binaries: the v1 server (main) fills a
// data folder through its website, then the v2 server starts on the same
// folder and everything must still be there and work.
//
//   AINKI_BIN=<v1 binary> UPGRADE_STEP=seed  npx playwright test -c playwright.upgrade.config.ts
//   E2E_KEEP_DATA=1       UPGRADE_STEP=check npx playwright test -c playwright.upgrade.config.ts
import { defineConfig, devices } from '@playwright/test';
import { baseURL } from './support/env.ts';

const step = process.env.UPGRADE_STEP ?? 'seed';
if (!['seed', 'check'].includes(step)) throw new Error(`UPGRADE_STEP must be seed or check, not ${step}`);

export default defineConfig({
  testDir: './upgrade',
  testMatch: step === 'seed' ? 'v1-seed.spec.ts' : 'v2-check.spec.ts',
  timeout: 60_000,
  workers: 1,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['list'], ['github']] : [['list']],
  use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 }, baseURL, timezoneId: 'Europe/Berlin', trace: 'retain-on-failure' },
  webServer: {
    command: 'node support/serve.ts',
    url: `${baseURL}/healthz`,
    reuseExistingServer: false,
    stdout: 'pipe',
    stderr: 'pipe',
    timeout: 60_000,
  },
});
