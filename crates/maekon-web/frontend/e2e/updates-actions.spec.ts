import { expect, test } from './helpers/test'

test.describe('Updates Actions', () => {
  test('P150: #section-status exists', async ({ page }) => {
    await page.goto('/updates/status')
    await expect(page.locator('#section-status')).toBeVisible()
  })

  test('P151: #section-channel exists', async ({ page }) => {
    await page.goto('/updates/channel')
    await expect(page.locator('#section-channel')).toBeVisible()
  })

  test('P152: version display exists', async ({ page }) => {
    await page.goto('/updates/status')
    const body = page.locator('body')
    await expect(body).toContainText(/\d+\.\d+\.\d+/)
  })
})
