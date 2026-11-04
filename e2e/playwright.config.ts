// Browser tests against the real server: the desktop (Calm) look, the
// simple look on a desktop, and phones, in Chromium and Firefox.
//
//   cargo build && cd e2e && npm ci && npx playwright test
//
// Firefox projects run where Firefox is installed (CI installs it; set
// E2E_BROWSERS=chromium to leave them out).
import { defineConfig, devices } from '@playwright/test';
import type { Look } from './support/fixtures.ts';
import { baseURL } from './support/env.ts';

const browsers = (process.env.E2E_BROWSERS ?? 'chromium,firefox').split(',');
const desk = { viewport: { width: 1440, height: 900 } };

const projects = [
  { name: 'desk-chromium', browser: 'chromium', use: { ...devices['Desktop Chrome'], ...desk, look: 'calm' as Look } },
  { name: 'simple-chromium', browser: 'chromium', use: { ...devices['Desktop Chrome'], ...desk, look: 'simple' as Look } },
  { name: 'phone-chromium', browser: 'chromium', use: { ...devices['Pixel 7'], look: 'calm' as Look } },
  { name: 'desk-firefox', browser: 'firefox', use: { ...devices['Desktop Firefox'], ...desk, look: 'calm' as Look } },
  // Firefox has no phone emulation; a narrow touch-less window still gets
  // the simple look, which is what phones see.
  { name: 'phone-firefox', browser: 'firefox', use: { ...devices['Desktop Firefox'], viewport: { width: 390, height: 844 }, look: 'calm' as Look } },
];

export default defineConfig({
  testDir: './tests',
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 4 : undefined,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }], ['github']] : [['list']],
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    timezoneId: 'Europe/Berlin',
    locale: 'en-US',
  },
  projects: projects
    .filter((p) => browsers.includes(p.browser))
    .map(({ name, use }) => ({ name, use })),
  webServer: {
    command: 'node support/serve.ts',
    url: `${baseURL}/healthz`,
    reuseExistingServer: false,
    stdout: 'pipe',
    stderr: 'pipe',
    timeout: 60_000,
  },
});
