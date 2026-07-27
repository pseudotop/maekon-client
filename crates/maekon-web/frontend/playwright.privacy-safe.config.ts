import { defineConfig, devices } from '@playwright/test'
import { DEFAULT_WEB_PORT } from './src/constants'

const previewHost = process.env.PLAYWRIGHT_PREVIEW_HOST || '127.0.0.1'
const previewPort = process.env.PLAYWRIGHT_PREVIEW_PORT || String(DEFAULT_WEB_PORT)
const managedBaseURL = `http://${previewHost}:${previewPort}`
const baseURL = process.env.PLAYWRIGHT_BASE_URL || managedBaseURL
const shouldManageWebServer = !process.env.PLAYWRIGHT_BASE_URL

export default defineConfig({
  testDir: './e2e',
  testMatch: 'privacy-safe-web-verification.spec.ts',
  fullyParallel: false,
  retries: 0,
  timeout: 30000,
  reporter: [['line']],
  outputDir: 'test-results/privacy-safe-web-verification',
  use: {
    baseURL,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
    headless: true,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
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
