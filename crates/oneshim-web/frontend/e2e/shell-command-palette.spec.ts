import { expect, test } from './helpers/test'

async function openPalette(page: import('@playwright/test').Page) {
  await page.getByTestId('titlebar-search').click()
  await expect(page.locator('[aria-modal="true"]')).toBeVisible()
}

test.describe('CommandPalette Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    await expect(page.getByTestId('titlebar-search')).toBeVisible()
  })

  test('P013: search button opens command palette', async ({ page }) => {
    await openPalette(page)
  })

  test('P014: search input filters commands', async ({ page }) => {
    await openPalette(page)
    const input = page.locator('input[role="combobox"]')
    await input.fill('settings')
    const options = page.locator('[id^="palette-option-"]')
    const count = await options.count()
    expect(count).toBeGreaterThanOrEqual(1)
    expect(count).toBeLessThan(15)
  })

  test('P015: Escape closes palette', async ({ page }) => {
    await openPalette(page)
    await page.keyboard.press('Escape')
    await expect(page.locator('[aria-modal="true"]')).not.toBeVisible()
  })

  test('P016: Enter on item navigates', async ({ page }) => {
    await openPalette(page)
    const input = page.locator('input[role="combobox"]')
    await input.fill('privacy')
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/privacy/)
  })

  test('P017: Arrow Down selects next', async ({ page }) => {
    await openPalette(page)
    await page.keyboard.press('ArrowDown')
    const input = page.locator('input[role="combobox"]')
    const activedesc = await input.getAttribute('aria-activedescendant')
    expect(activedesc).toBeTruthy()
  })

  test('P018: focus trap keeps Tab within dialog', async ({ page }) => {
    // Focus trap now catches the "focus drifted outside the dialog" case,
    // so Tab is redirected back to the first focusable inside. With only
    // the input focusable inside the dialog, repeated Tab presses should
    // keep focus pinned to the combobox.
    await openPalette(page)
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press('Tab')
    }
    const modal = page.locator('[aria-modal="true"]')
    const activeInModal = await modal.evaluate((m) => m.contains(document.activeElement))
    expect(activeInModal).toBe(true)
  })
})
