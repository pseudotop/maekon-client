import { expect, test } from './helpers/test'

test.describe('Reports Actions', () => {
  test('P069: period-week button exists', async ({ page }) => {
    await page.goto('/reports/activity')
    const btn = page.getByTestId('period-week')
    await expect(btn).toBeVisible()
  })

  test('P070: period-month button exists', async ({ page }) => {
    await page.goto('/reports/activity')
    const btn = page.getByTestId('period-month')
    await expect(btn).toBeVisible()
  })

  test('P071: period-custom button exists', async ({ page }) => {
    await page.goto('/reports/activity')
    const btn = page.getByTestId('period-custom')
    await expect(btn).toBeVisible()
  })

  test('P072: generate-report button exists', async ({ page }) => {
    await page.goto('/reports/activity')
    await page.getByTestId('period-custom').click()
    const btn = page.getByTestId('generate-report')
    await expect(btn).toBeVisible()
  })

  test('P073: clicking period button changes active state', async ({ page }) => {
    await page.goto('/reports/activity')
    const btn = page.getByTestId('period-month')
    await btn.click()
    await expect(btn).toHaveClass(/bg-brand/)
  })

  test('P074: #section-activity exists', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.locator('#section-activity')).toBeVisible()
  })

  test('P075: #section-focus exists', async ({ page }) => {
    await page.goto('/reports/focus')
    await expect(page.locator('#section-focus')).toBeVisible()
  })
})
