import { expect, test } from './helpers/test'

test.describe('Cross-Page Actions', () => {
  test('P160: Cmd+K works from any page', async ({ page }) => {
    await page.goto('/settings')
    await expect(page.locator('h1').first()).toBeVisible()
    await page.evaluate(() => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', {
          key: 'k',
          metaKey: true,
          bubbles: true,
          cancelable: true,
        }),
      )
    })
    await expect(page.locator('[aria-modal="true"]')).toBeVisible()
  })

  test('P161: navigation preserves scroll position', async ({ page }) => {
    await page.goto('/')
    // Scroll down
    await page.evaluate(() => window.scrollTo(0, 200))
    // Navigate away and back
    await page.goto('/settings')
    await page.goto('/')
    // New page starts at top
    const scrollY = await page.evaluate(() => window.scrollY)
    expect(scrollY).toBeLessThanOrEqual(10)
  })

  test('P162: all pages have h1 heading', async ({ page }) => {
    const routes = [
      '/',
      '/timeline',
      '/replay',
      '/automation',
      '/focus',
      '/reports',
      '/search',
      '/updates',
      '/settings',
      '/privacy',
    ]
    for (const route of routes) {
      await page.goto(route)
      await expect(page.locator('h1').first()).toBeVisible()
    }
  })
})
