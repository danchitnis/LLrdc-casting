import { defineConfig } from '@playwright/test';

const artifactDir = process.env.E2E_ARTIFACT_DIR || '../.artefact/manual';

export default defineConfig({
  testDir: './e2e',
  outputDir: `${artifactDir}/playwright-output`,
  // The local suite intentionally runs ten independent hardware cycles.
  timeout: 600_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [
    ['line'],
    ['html', { outputFolder: `${artifactDir}/playwright-report`, open: 'never' }],
  ],
  use: {
    browserName: 'chromium',
    channel: 'chrome',
    headless: false,
    ignoreHTTPSErrors: true,
    actionTimeout: 15_000,
    navigationTimeout: 45_000,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
  projects: [
    {
      name: 'management-admin',
      testMatch: /admin\.spec\.ts/,
    },
    {
      name: 'hardware-codec',
      testMatch: /hardware\.spec\.ts/,
    },
    {
      name: 'hardware-cloud',
      testMatch: /cloud\.spec\.ts/,
    },
  ],
});
