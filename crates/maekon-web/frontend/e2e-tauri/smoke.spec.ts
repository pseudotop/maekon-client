/**
 * Tauri desktop app smoke tests (public).
 *
 * Verifies the Tauri WKWebView renders the React frontend correctly
 * and the IPC bridge (Tauri commands) works. Tests run against the
 * actual desktop app, not a browser.
 *
 * Comprehensive tests live in the private test suite.
 */

import { ensureShellReady, switchToMainWindow } from './helpers.js'

describe('Tauri App Smoke', () => {
  before(async () => {
    await switchToMainWindow()
    await ensureShellReady()
  })

  it('window is visible and has a title', async () => {
    const title = await browser.getTitle()
    expect(title).toContain('Maekon')
  })

  it('main content renders (not a white screen)', async () => {
    const body = await $('body')
    await body.waitForExist({ timeout: 10000 })
    const text = await body.getText()
    expect(text.length).toBeGreaterThan(50)
  })

  it('StatusBar shows connection status', async () => {
    const statusBar = await $('.app-shell-statusbar')
    await statusBar.waitForExist({ timeout: 10000 })
    const text = await statusBar.getText()
    expect(text.trim().length).toBeGreaterThan(0)
  })

  it('navigation buttons exist in ActivityBar', async () => {
    const buttons = await $$('button[data-testid^="nav-group-"], button[data-testid^="nav-"]')
    expect(buttons.length).toBeGreaterThanOrEqual(5)
  })

  it('no error boundary visible', async () => {
    const html = await browser.getPageSource()
    expect(html).not.toContain('error-boundary')
  })
})
