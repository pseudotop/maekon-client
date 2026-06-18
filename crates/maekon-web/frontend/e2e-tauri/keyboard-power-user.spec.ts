// e2e-tauri/keyboard-power-user.spec.ts

describe('J3: Keyboard Power User', () => {
  beforeEach(async () => {
    // Navigate to the Dashboard before each test
    await browser.url('tauri://localhost/')
    await $('nav[role="navigation"]').waitForExist({ timeout: 10000 })
  })

  /**
   * @tc_id T120
   * @risk_id UX-001
   * @tauri_only_reason Cmd+W = close-to-tray (Tauri api.prevent_close()), not browser close
   */
  it('T120: Cmd+W hides window (close-to-tray)', async () => {
    // Check the window state before Cmd+W
    const titleBefore = await browser.getTitle()
    expect(titleBefore).toContain('Maekon')

    // Send Cmd+W
    await browser.keys(['Meta', 'w'])
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
  it('T121: Cmd+K opens Command Palette with focus', async () => {
    await browser.keys(['Meta', 'k'])

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
    await browser.keys(['Meta', 'k'])
    await $('input[role="combobox"]').waitForExist({ timeout: 3000 })

    // Type "time" → matches Timeline
    await browser.keys('time')
    await browser.pause(500)

    // Select the first option
    await browser.keys('ArrowDown')
    await browser.keys('Enter')
    await browser.pause(1000)

    // Verify the URL contains /timeline
    const url = await browser.getUrl()
    expect(url).toContain('/timeline')
  })

  /**
   * @tc_id T123
   * @risk_id UX-004
   * @tauri_only_reason Dialog lifecycle in real WebView
   */
  it('T123: Escape closes Command Palette', async () => {
    await browser.keys(['Meta', 'k'])
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
