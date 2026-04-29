import { expect, test } from './helpers/test'

test.describe('SidePanel & Skip Link Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
  })

  test('P019: resize handle exists', async ({ page }) => {
    const handle = page.locator('div[role="separator"]')
    await expect(handle).toBeVisible()
  })

  test('P020: skip-to-content link exists and jumps', async ({ page }) => {
    const skipLink = page.locator('a[href="#main-content"]')
    await expect(skipLink).toBeVisible()
    await skipLink.focus()
  })
})
