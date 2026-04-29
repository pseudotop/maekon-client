/**
 * WebdriverIO configuration for Tauri E2E tests.
 *
 * Tests the actual Tauri desktop app (WKWebView on macOS)
 * via the embedded tauri-plugin-webdriver W3C server.
 *
 * Prerequisites:
 *   cargo build -p oneshim-app --features webdriver
 */
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { startApp, stopApp } from './app-launcher.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const WEBDRIVER_PORT = parseInt(process.env.TAURI_WEBDRIVER_PORT ?? '4445', 10)

export const config = {
  runner: 'local',

  // Connect directly to the embedded WebDriver server (no intermediary)
  hostname: '127.0.0.1',
  port: WEBDRIVER_PORT,
  path: '/',

  specs: [`${__dirname}/**/*.spec.ts`],

  maxInstances: 1,

  capabilities: [
    {
      // W3C WebDriver capabilities — no browser needed
      browserName: 'wry',
      'wry:options': {},
    },
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

  // App lifecycle management
  onPrepare: async () => {
    await startApp(WEBDRIVER_PORT)
  },

  onComplete: () => {
    stopApp()
  },
}
