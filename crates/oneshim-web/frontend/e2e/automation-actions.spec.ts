import { expect, test } from './helpers/test'

test.describe('Automation Actions', () => {
  // P093-P097 (productivity / appmanagement / workflow / custom tab tests) were
  // removed when /automation was refactored from a single tabbed page into
  // sub-routes. The automation page now exposes only /automation/policies,
  // /automation/commands and /automation/history — covered by P098-P100 below.

  test('P098: #section-history exists', async ({ page }) => {
    await page.goto('/automation/history')
    await expect(page.locator('#section-history')).toBeVisible()
  })

  test('P099: #section-commands exists', async ({ page }) => {
    await page.goto('/automation/commands')
    await expect(page.locator('#section-commands')).toBeVisible()
  })

  test('P100: #section-policies exists', async ({ page }) => {
    await page.goto('/automation/policies')
    await expect(page.locator('#section-policies')).toBeVisible()
  })
})
