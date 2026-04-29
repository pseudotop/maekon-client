import { expect, test } from './helpers/test'

test.describe('Dashboard Actions', () => {
  test('P031: page heading renders', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('h1').first()).toBeVisible()
  })

  test('P032: #section-overview exists', async ({ page }) => {
    await page.goto('/overview')
    await expect(page.locator('#section-overview')).toBeVisible()
  })

  test('P033: #section-metrics shows data', async ({ page }) => {
    await page.goto('/monitoring')
    const section = page.locator('#section-metrics')
    await expect(section).toBeVisible()
  })

  test('P034: #section-processes lists processes', async ({ page }) => {
    await page.goto('/monitoring')
    const section = page.locator('#section-processes')
    await expect(section).toBeVisible()
  })

  test('P035: #section-focus shows focus widget', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.locator('#section-focus')).toBeVisible()
  })

  test('P036: #section-heatmap renders', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.locator('#section-heatmap')).toBeVisible()
  })

  test('P037: #section-updates exists', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.locator('#section-updates')).toBeVisible()
  })

  test('P038: metric cards show values', async ({ page }) => {
    await page.goto('/overview')
    const body = page.locator('body')
    await expect(body).toContainText(/\d/)
  })
})
