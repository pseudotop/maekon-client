import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'

const monitorName = i18nRegex('nav.groupMonitor')
const insightsName = i18nRegex('nav.groupInsights')
const manageName = i18nRegex('nav.groupManage')
const settingsName = i18nRegex('nav.settings')
const privacyName = i18nRegex('nav.privacy')
const shortcutsTitle = i18nRegex('shortcuts.title')

test.describe('Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    // ActivityBar uses buttons with title attribute for navigation.
    await expect(page.locator('nav')).toBeVisible()
  })

  test('ActivityBar exposes the category icons + bottom direct icons', async ({ page }) => {
    // After the category restructure the 48px rail holds just 3 group icons
    // for main navigation and 2 direct icons for Settings / Privacy.  Every
    // sub-route is reached via the SidePanel tree.
    await expect(page.getByTitle(monitorName)).toBeVisible()
    await expect(page.getByTitle(insightsName)).toBeVisible()
    await expect(page.getByTitle(manageName)).toBeVisible()
    await expect(page.getByTitle(settingsName)).toBeVisible()
    await expect(page.getByTitle(privacyName)).toBeVisible()
  })

  test('clicking Monitor lands on the dashboard overview', async ({ page }) => {
    await page.getByTitle(monitorName).click()
    await expect(page).toHaveURL(/\/overview$/)
  })

  test('clicking Data lands on the reports activity view', async ({ page }) => {
    await page.getByTitle(insightsName).click()
    await expect(page).toHaveURL(/\/reports\/activity$/)
  })

  test('clicking Manage lands on automation', async ({ page }) => {
    await page.getByTitle(manageName).click()
    await expect(page).toHaveURL(/\/automation\/policies$/)
  })

  test('should navigate to Settings', async ({ page }) => {
    await page.getByTitle(settingsName).click()
    await expect(page).toHaveURL(/\/settings\/general$/)
  })

  test('should navigate to Privacy', async ({ page }) => {
    await page.getByTitle(privacyName).click()
    await expect(page).toHaveURL(/\/privacy\/data$/)
  })

  test('SidePanel tree navigates to any leaf in the active group', async ({ page }) => {
    // beforeEach already lands on / (Monitor group) so the SidePanel tree is
    // already visible — clicking Monitor again would toggle the panel rather
    // than navigate.  Use the existing tree to drill into Timeline > All
    // Frames, which proves the ActivityBar + SidePanel pair covers every
    // route that used to have its own rail icon.
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()
    await tree.getByRole('treeitem', { name: /all frames/i }).click()
    await expect(page).toHaveURL(/\/timeline\/all$/)
  })

  test('should show keyboard shortcuts help with ? key', async ({ page }) => {
    await page.locator('body').click()
    await page.keyboard.press('Shift+Slash')

    const heading = page.getByRole('heading', {
      name: shortcutsTitle,
    })
    await expect(heading).toBeVisible({ timeout: 10000 })
  })

  test('should navigate with keyboard shortcuts', async ({ page }) => {
    await page.keyboard.press('d')
    await expect(page).toHaveURL(/\/overview$/)

    await page.keyboard.press('t')
    await expect(page).toHaveURL(/\/timeline\/all$/)

    await page.keyboard.press('s')
    await expect(page).toHaveURL(/\/settings\/general$/)

    await page.keyboard.press('p')
    await expect(page).toHaveURL(/\/privacy\/data$/)
  })
})
