import { expect, test } from './helpers/test'

test.describe('Search Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/search')
  })

  test('P056: search input exists', async ({ page }) => {
    const input = page.getByTestId('search-input')
    await expect(input).toBeVisible()
  })

  test('P057: search input accepts text', async ({ page }) => {
    const input = page.getByTestId('search-input')
    await input.fill('test query')
    await expect(input).toHaveValue('test query')
  })

  test('P058: filter-all button exists', async ({ page }) => {
    const btn = page.getByTestId('filter-all')
    await expect(btn).toBeVisible()
  })

  test('P059: filter-frames button exists', async ({ page }) => {
    const btn = page.getByTestId('filter-frames')
    await expect(btn).toBeVisible()
  })

  test('P060: filter-events button exists', async ({ page }) => {
    const btn = page.getByTestId('filter-events')
    await expect(btn).toBeVisible()
  })

  test('P061: submit button exists', async ({ page }) => {
    const btn = page.locator('button[type="submit"]')
    await expect(btn).toBeVisible()
  })
})
