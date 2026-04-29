import { expect, test } from './helpers/test'

test.describe('Focus Actions', () => {
  test('P108: #section-score exists', async ({ page }) => {
    await page.goto('/focus/score')
    await expect(page.locator('#section-score')).toBeVisible()
  })

  test('P109: gauge SVG renders', async ({ page }) => {
    await page.goto('/focus/score')
    const svg = page.locator('#section-score svg[viewBox="0 0 100 100"]')
    await expect(svg).toBeVisible()
  })

  test('P110: score shows numeric value', async ({ page }) => {
    await page.goto('/focus/score')
    const score = page.locator('#section-score')
    await expect(score).toContainText(/\d/)
  })

  test('P111: #section-trend exists', async ({ page }) => {
    await page.goto('/focus/score')
    await expect(page.locator('#section-trend')).toBeVisible()
  })

  test('P112: #section-sessions exists', async ({ page }) => {
    await page.goto('/focus/sessions')
    await expect(page.locator('#section-sessions')).toBeVisible()
  })

  test('P113: #section-interruptions exists', async ({ page }) => {
    await page.goto('/focus/interruptions')
    await expect(page.locator('#section-interruptions')).toBeVisible()
  })
})
