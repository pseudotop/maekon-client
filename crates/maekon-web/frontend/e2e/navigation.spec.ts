import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'

const mainName = i18nRegex('nav.groupMain')
const settingsName = i18nRegex('nav.settings')
const privacyName = i18nRegex('nav.privacy')
const shortcutsTitle = i18nRegex('shortcuts.title')

test.describe('Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    // ActivityBar uses buttons with title attribute for navigation.
    await expect(page.locator('nav')).toBeVisible()
  })

  test('ActivityBar exposes the main group icon + bottom direct icons', async ({ page }) => {
    // #13736: the 48px rail holds one group icon for the main tree and 2
    // direct icons for Settings / Privacy.  Every other route is reached via
    // the SidePanel tree.
    await expect(page.getByTitle(mainName)).toBeVisible()
    await expect(page.getByTitle(settingsName)).toBeVisible()
    await expect(page.getByTitle(privacyName)).toBeVisible()
  })

  test('clicking the main group icon from Settings lands on the dashboard overview', async ({ page }) => {
    await page.getByTitle(settingsName).click()
    await expect(page).toHaveURL(/\/settings\/general$/)
    await page.getByTitle(mainName).click()
    await expect(page).toHaveURL(/\/overview$/)
  })

  test('a destination moved into a bundle opens one level back', async ({ page }) => {
    // Automation left the first level for the Tools bundle; unfolding the
    // bundle and clicking it lands on its default tab.
    const tree = page.locator('[role="tree"]')
    await tree.getByRole('treeitem', { name: i18nRegex('nav.sectionTools') }).click()
    await tree
      .getByRole('treeitem', { name: i18nRegex('nav.automation') })
      .first()
      .click()
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

  test('SidePanel tree navigates to a nested leaf of a front destination', async ({ page }) => {
    // beforeEach already lands on / (main group) so the SidePanel tree is
    // already visible — clicking the group icon again would toggle the panel
    // rather than navigate.  Timeline starts folded (only the path to the
    // current page is open), so unfold it and drill into Filters.
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()
    await tree
      .getByRole('treeitem', { name: i18nRegex('nav.timeline') })
      .first()
      .click()
    await tree.getByRole('treeitem', { name: i18nRegex('sidebar.filters') }).click()
    await expect(page).toHaveURL(/\/timeline\/filters$/)
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
