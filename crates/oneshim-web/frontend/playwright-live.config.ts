/**
 * Playwright config for live E2E tests against the running Maekon app.
 *
 * Unlike the default config (which uses mock APIs + pnpm preview),
 * this connects to the actual Rust backend and verifies real behavior.
 *
 * Usage:
 *   1. Start the app: cargo run (or cargo tauri dev)
 *   2. Run tests:     pnpm test:e2e:live
 *   3. Headed mode:   pnpm test:e2e:live:headed
 */
import { defineConfig, devices } from '@playwright/test'
import { DEFAULT_WEB_PORT } from './src/constants'

const port = Number(process.env.ONESHIM_PORT || DEFAULT_WEB_PORT)
const baseURL = `http://127.0.0.1:${port}`

export default defineConfig({
  testDir: './e2e-live',

  testMatch: '**/*.spec.ts',

  fullyParallel: false, // Sequential — tests share a live backend

  retries: 0,

  timeout: 15000,

  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-live-report' }]],

  outputDir: 'test-results-live',

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
  ],

  // No webServer — expects the app to already be running
})
