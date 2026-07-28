/**
 * WebdriverIO configuration for Tauri E2E tests.
 *
 * Tests the actual Tauri desktop app through the official WebdriverIO Tauri
 * service and its cross-platform embedded provider.
 *
 * Prerequisites:
 *   cargo build -p maekon-app --features webdriver
 */
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { findBinary, prepareE2eEnvironment } from './app-launcher.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const WEBDRIVER_PORT = parseInt(process.env.TAURI_WEBDRIVER_PORT ?? '4445', 10)
const APP_BINARY = findBinary()
const APP_ENV = prepareE2eEnvironment()

export const config = {
  runner: 'local',

  // Every spec mutates or observes the same desktop process and profile. Group
  // the files into one worker so WebdriverIO cannot create parallel sessions.
  specs: [[`${__dirname}/**/*.spec.ts`]],

  maxInstances: 1,
  maxInstancesPerCapability: 1,

  capabilities: [
    {
      browserName: 'tauri',
      'tauri:options': {
        application: APP_BINARY,
      },
      'wdio:maxInstances': 1,
    },
  ],

  services: [
    [
      '@wdio/tauri-service',
      {
        appBinaryPath: APP_BINARY,
        driverProvider: 'embedded',
        embeddedPort: WEBDRIVER_PORT,
        env: APP_ENV,
        startTimeout: 60000,
        statusPollTimeout: 5000,
        captureBackendLogs: true,
        // The app's privacy-filtered frontend bridge is canonical. WebView2
        // console capture in service 1.1.0 rejects nullable source positions.
        captureFrontendLogs: false,
        backendLogLevel: 'warn',
        frontendLogLevel: 'warn',
      },
    ],
  ],

  logLevel: 'warn',
  waitforTimeout: 10000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  framework: 'mocha',
  mochaOpts: {
    ui: 'bdd',
    timeout: 30000,
  },

  reporters: ['spec'],

  before: async () => {
    const { ensureShellReady, switchToMainWindow } = await import('./helpers.js')
    await switchToMainWindow()
    await ensureShellReady()
  },
}
