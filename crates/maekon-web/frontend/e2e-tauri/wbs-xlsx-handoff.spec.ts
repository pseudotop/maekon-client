import { navigateMain } from './helpers.js'

const primaryModifier = process.platform === 'darwin' ? 'Meta' : 'Control'

describe('WD-04.5: WBS XLSX handoff', () => {
  beforeEach(async () => {
    await navigateMain('/')
    await $('nav[aria-label]').waitForExist({ timeout: 10000 })
  })

  /**
   * @tc_id CRT-PRV-WBS-XLSX-001
   * @risk_id HANDOFF-001
   * @tauri_only_reason Command Palette routing and receipt query hydration run in the real Tauri WebView
   */
  it('opens the synthetic XLSX draft from the palette and hydrates an exact receipt link', async () => {
    await browser.keys([primaryModifier, 'k'])
    const paletteOption = await $('[id="palette-option-route-/wbs-xlsx-draft"]')
    await paletteOption.waitForExist({ timeout: 3000 })
    await paletteOption.click()

    const page = await $('[data-testid="wbs-xlsx-draft"]')
    await page.waitForExist({ timeout: 5000 })
    expect(await browser.getUrl()).toContain('/wbs-xlsx-draft')
    expect(await (await $('[data-testid="wbs-xlsx-synthetic-badge"]')).isDisplayed()).toBe(true)

    const receiptId = 'ercv-synthetic-wd-brokerage-001'
    await navigateMain(`/wbs-xlsx-draft?receipt=${receiptId}`)

    const receiptInput = await $('[data-testid="wbs-xlsx-draft"] input')
    await receiptInput.waitForExist({ timeout: 5000 })
    expect(await receiptInput.getValue()).toBe(receiptId)
    expect(await (await $('[data-testid="wbs-xlsx-synthetic-badge"]')).isDisplayed()).toBe(true)
  })
})
