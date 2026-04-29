import { expect, test } from './helpers/test'

test.describe('Replay Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/replay')
  })

  test('P079: play/pause button exists', async ({ page }) => {
    const btn = page.getByTestId('replay-play')
    await expect(btn).toBeVisible()
  })

  test('P080: speed buttons exist', async ({ page }) => {
    const speed = page.getByRole('combobox')
    await expect(speed).toBeVisible()
  })

  test('P081: skip to start button exists', async ({ page }) => {
    const btn = page.getByTestId('replay-start')
    await expect(btn).toBeVisible()
  })

  test('P082: skip to end button exists', async ({ page }) => {
    const btn = page.getByTestId('replay-end')
    await expect(btn).toBeVisible()
  })

  test('P083: overlay toggle exists', async ({ page }) => {
    const btn = page.getByTestId('overlay-toggle')
    await expect(btn).toBeVisible()
  })

  test('P084: #section-events exists', async ({ page }) => {
    await page.goto('/replay/events')
    const section = page.locator('#section-events')
    await expect(section).toBeVisible()
  })
})
