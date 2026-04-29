/**
 *
 */
import { defineConfig, devices } from '@playwright/test'
import { DEFAULT_WEB_PORT } from './src/constants'

const previewHost = process.env.PLAYWRIGHT_PREVIEW_HOST || '127.0.0.1'
const previewPort = process.env.PLAYWRIGHT_PREVIEW_PORT || String(DEFAULT_WEB_PORT)
const managedBaseURL = `http://${previewHost}:${previewPort}`
const baseURL = process.env.PLAYWRIGHT_BASE_URL || managedBaseURL
const shouldManageWebServer = !process.env.PLAYWRIGHT_BASE_URL

export default defineConfig({
  testDir: './e2e',

  testMatch: '**/*.spec.ts',

  fullyParallel: true,

  // Local runs match CI retry budget (2). Without this, parallel-worker
  // contention against the vite preview server occasionally hides random
  // tests behind a transient network/hydration race that disappears on rerun.
  retries: 2,

  timeout: 30000,

  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],

  outputDir: 'test-results',

  use: {
    baseURL,

    trace: 'on-first-retry',

    screenshot: 'only-on-failure',

    headless: true,
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    // {
    //   name: 'firefox',
    //   use: { ...devices['Desktop Firefox'] },
    // },
    // {
    //   name: 'webkit',
    //   use: { ...devices['Desktop Safari'] },
    // },
  ],

  ...(shouldManageWebServer
    ? {
        webServer: {
          command: `pnpm preview --host ${previewHost} --port ${previewPort}`,
          url: managedBaseURL,
          reuseExistingServer: !process.env.CI,
        },
      }
    : {}),
})
