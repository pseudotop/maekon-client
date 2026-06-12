/**
 * Tauri desktop app smoke tests (public).
 *
 * Verifies the Tauri WKWebView renders the React frontend correctly
 * and the IPC bridge (Tauri commands) works. Tests run against the
 * actual desktop app, not a browser.
 *
 * Comprehensive tests live in the private test suite.
 */

import { invokeIpc } from './helpers.js'

async function switchToMainWindow(): Promise<void> {
  await browser.waitUntil(
    async () => {
      const handles = await browser.getWindowHandles()
      for (const handle of handles) {
        await browser.switchToWindow(handle)
        const title = await browser.getTitle().catch(() => '')
        const url = await browser.getUrl().catch(() => '')
        const isAuxiliaryWindow = url.includes('overlay') || url.includes('tracking-panel')
        if (title.includes('Maekon') && !isAuxiliaryWindow) return true
      }
      return false
    },
    { timeout: 15000, timeoutMsg: 'Maekon main window was not available' },
  )
}

async function ensureShellReady(): Promise<void> {
  const body = await $('body')
  await body.waitForExist({ timeout: 10000 })

  try {
    await browser.waitUntil(
      async () => {
        const statusBar = await $('.app-shell-statusbar')
        const skipButton = await $('[data-testid="onboarding-skip"]')
        return (await statusBar.isExisting()) || (await skipButton.isExisting())
      },
      { timeout: 15000, timeoutMsg: 'App shell or onboarding did not render' },
    )
  } catch (error) {
    const title = await browser.getTitle().catch(() => '<title unavailable>')
    const url = await browser.getUrl().catch(() => '<url unavailable>')
    const source = await browser.getPageSource().catch(() => '<source unavailable>')
    console.log(`[e2e-tauri] Render timeout title=${title} url=${url} source=${source.slice(0, 500)}`)
    throw error
  }

  if (await (await $('.app-shell-statusbar')).isExisting()) return

  const skipButton = await $('[data-testid="onboarding-skip"]')
  if (await skipButton.isExisting()) {
    await skipButton.waitForClickable({ timeout: 10000 })
    await skipButton.click()
  } else {
    await invokeIpc('complete_onboarding')
    await browser.execute(() => window.location.reload())
  }
  await $('.app-shell-statusbar').waitForExist({ timeout: 15000 })
}

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
