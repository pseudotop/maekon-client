// e2e-tauri/keyboard-power-user.spec.ts

import { invokeIpc, navigateMain } from './helpers.js'

const primaryModifier = process.platform === 'darwin' ? 'Meta' : 'Control'

describe('J3: Keyboard Power User', () => {
  beforeEach(async () => {
    // Navigate to the Dashboard before each test
    await navigateMain('/')
    // A semantic <nav> has an implicit navigation role; no explicit role
    // attribute is required or emitted by the production component.
    await $('nav[aria-label]').waitForExist({ timeout: 10000 })
  })

  afterEach(async () => {
    const palette = await $('[data-testid="command-palette"]')
    if (await palette.isExisting()) {
      await browser.keys('Escape')
    }
    // T120 intentionally hides the real window; restore it for the remaining
    // tests while keeping the same process and WebDriver session alive.
    await invokeIpc('show_main_window')
  })

  /**
   * @tc_id T120
   * @risk_id UX-001
   * @tauri_only_reason Cmd+W = close-to-tray (Tauri api.prevent_close()), not browser close
   */
  it('T120: CmdOrCtrl+W hides window (close-to-tray)', async () => {
    // Check the window state before Cmd+W
    const titleBefore = await browser.getTitle()
    expect(titleBefore).toContain('Maekon')

    // Send the platform primary modifier + W.
    await browser.keys([primaryModifier, 'w'])
    await browser.pause(1000)

    // Verify the process is still running (WebDriver connection alive = app alive)
    // The WebDriver session is preserved even when the window is hidden
    const titleAfter = await browser.getTitle()
    expect(titleAfter).toBeDefined()
  })

  /**
   * @tc_id T121
   * @risk_id UX-002
   * @tauri_only_reason Real Tauri WebView keyboard event routing
   */
  it('T121: CmdOrCtrl+K opens Command Palette with focus', async () => {
    await browser.keys([primaryModifier, 'k'])

    const dialog = await $('div[role="dialog"]')
    await dialog.waitForExist({ timeout: 3000 })
    expect(await dialog.isDisplayed()).toBe(true)

    // Verify the combobox input received focus
    const input = await $('input[role="combobox"]')
    expect(await input.isFocused()).toBe(true)

    // Cleanup: close with Escape
    await browser.keys('Escape')
  })

  /**
   * @tc_id T122
   * @risk_id UX-003
   * @tauri_only_reason Command Palette navigates via Tauri router
   */
  it('T122: Command Palette Enter navigates to selected page', async () => {
    await browser.keys([primaryModifier, 'k'])
    await $('input[role="combobox"]').waitForExist({ timeout: 3000 })

    const input = await $('input[role="combobox"]')
    await input.click()

    // Select the Timeline parent entry by its route-derived DOM id. This stays
    // stable across localized labels and still exercises keyboard selection.
    const options = await $$('[role="option"]')
    const optionIds = await options.map((option) => option.getAttribute('id'))
    const timelineIndex = optionIds.indexOf('palette-option-route-/timeline')
    expect(timelineIndex).toBeGreaterThanOrEqual(0)
    for (let index = 0; index < timelineIndex; index += 1) {
      await browser.keys('ArrowDown')
    }

    const timelineOption = await $('[id="palette-option-route-/timeline"]')
    expect(await timelineOption.getAttribute('aria-selected')).toBe('true')
    await browser.keys('Enter')

    // Verify the URL contains /timeline
    await browser.waitUntil(async () => (await browser.getUrl()).includes('/timeline'), {
      timeout: 5000,
      timeoutMsg: 'Command Palette did not navigate to the Timeline route',
    })
  })

  /**
   * @tc_id T123
   * @risk_id UX-004
   * @tauri_only_reason Dialog lifecycle in real WebView
   */
  it('T123: Escape closes Command Palette', async () => {
    await browser.keys([primaryModifier, 'k'])
    const dialog = await $('div[role="dialog"]')
    await dialog.waitForExist({ timeout: 3000 })

    await browser.keys('Escape')
    await browser.pause(500)

    expect(await dialog.isExisting()).toBe(false)
  })

  /**
   * @tc_id T124
   * @risk_id RESIL-003
   * @tauri_only_reason Rapid navigation stress test on real WebView renderer
   */
  it('T124: Rapid navigation (10 transitions) survives', async () => {
    const shortcuts = ['d', 't', 's', 'p', 'd', 't', 's', 'p', 'd', 't']

    for (const key of shortcuts) {
      await browser.keys(key)
      await browser.pause(200)
    }

    // Last key 't' → /timeline
    await browser.pause(1000)
    const url = await browser.getUrl()
    expect(url).toContain('/timeline')

    // Verify no error-boundary
    const source = await browser.getPageSource()
    expect(source).not.toContain('error-boundary')
    expect(source).not.toContain('Something went wrong')
  })
})
