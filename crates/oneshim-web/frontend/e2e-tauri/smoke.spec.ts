/**
 * Tauri desktop app smoke tests (public).
 *
 * Verifies the Tauri WKWebView renders the React frontend correctly
 * and the IPC bridge (Tauri commands) works. Tests run against the
 * actual desktop app, not a browser.
 *
 * Comprehensive tests live in the private test suite.
 */

describe('Tauri App Smoke', () => {
  it('window is visible and has a title', async () => {
    const title = await browser.getTitle()
    expect(title).toContain('ONESHIM')
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
    // Should show either "Connected" or Korean equivalent
    expect(text).toMatch(/connected|연결됨|offline|오프라인/i)
  })

  it('navigation buttons exist in ActivityBar', async () => {
    const nav = await $('nav[role="navigation"]')
    await nav.waitForExist({ timeout: 5000 })
    const buttons = await nav.$$('button')
    expect(buttons.length).toBeGreaterThanOrEqual(5)
  })

  it('no error boundary visible', async () => {
    const html = await browser.getPageSource()
    expect(html).not.toContain('error-boundary')
  })
})
