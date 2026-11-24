import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'

test.describe('ActivityBar Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
  })

  // #13736: the ActivityBar exposes one main group icon for the main tree and
  // 2 direct icons (settings/privacy) at the bottom.  Clicking the group icon
  // navigates to the dashboard; every other route is reached through the
  // SidePanel tree (six front destinations plus the More / Tools / System
  // bundles), not from the ActivityBar.
  const GROUP_BUTTONS = [{ id: 'nav-group-main', expectedUrl: /\/overview$/ }]

  const BOTTOM_BUTTONS = [
    { id: 'nav-settings', expectedUrl: /\/settings\/general$/ },
    { id: 'nav-privacy', expectedUrl: /\/privacy\/data$/ },
  ]

  for (const item of [...GROUP_BUTTONS, ...BOTTOM_BUTTONS]) {
    test(`P00x-${item.id}: nav button exists and navigates`, async ({ page }) => {
      const btn = page.getByTestId(item.id)
      await expect(btn).toBeVisible()
      await btn.click()
      await expect(page).toHaveURL(item.expectedUrl)
    })
  }

  test('P011: only one button has aria-current="page"', async ({ page }) => {
    await page.getByTestId('nav-group-main').click()
    const activeButtons = page.locator('nav button[aria-current="page"]')
    await expect(activeButtons).toHaveCount(1)
  })

  test('P012: tooltip appears on hover', async ({ page }) => {
    const btn = page.getByTestId('nav-settings')
    await btn.hover()
    const tooltip = page.locator('#activity-bar-tooltip')
    await expect(tooltip).toBeVisible()
    await expect(tooltip).toHaveText(/.+/)
  })

  test('P013: main group highlights on a route inside a bundle', async ({ page }) => {
    // /automation/policies sits in the Tools bundle one level back, and the
    // main group icon is still the active one.
    await page.goto('/automation/policies')
    const active = page.locator('nav button[aria-current="page"]')
    await expect(active).toHaveCount(1)
    await expect(active).toHaveAttribute('data-testid', 'nav-group-main')
  })

  test('P014: activity bar exposes exactly three nav buttons', async ({ page }) => {
    // 1 group icon + 2 bottom direct icons (settings, privacy) = 3 total.
    // Support moved into the System bundle (#13736).  Guards against
    // accidental re-introduction of per-route icons in the 48px rail.
    const nav = page.locator('nav[aria-label]')
    const buttons = nav.locator('button[data-testid]')
    await expect(buttons).toHaveCount(3)
  })

  test('P015: clicking the active group toggles the SidePanel', async ({ page }) => {
    await page.goto('/overview')
    // Sidepanel visible initially (on /overview which is in the main group).
    const sidePanel = page.locator('[role="tree"]')
    await expect(sidePanel).toBeVisible()

    // Click the already-active group icon → sidepanel should collapse.
    await page.getByTestId('nav-group-main').click()
    await expect(sidePanel).toHaveCount(0)

    // Click again → sidepanel re-opens.
    await page.getByTestId('nav-group-main').click()
    await expect(sidePanel).toBeVisible()
  })

  test('P016: SidePanel opens the bundle that holds the current page', async ({ page }) => {
    await page.goto('/automation/policies')
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()

    // The Tools bundle opens to reveal Automation and its tabs; the other
    // bundles stay folded.  Tab labels come from i18n (sidebar.policies →
    // "Runtime Status"), NOT from the route path segments.
    await expect(tree.getByRole('treeitem', { name: i18nRegex('nav.sectionTools') })).toHaveAttribute(
      'aria-expanded',
      'true',
    )
    await expect(tree.getByRole('treeitem', { name: i18nRegex('nav.sectionMore') })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
    await expect(tree.getByRole('treeitem', { name: i18nRegex('nav.automation') }).first()).toBeVisible()
    await expect(tree.getByRole('treeitem', { name: i18nRegex('nav.audit') })).toBeVisible()

    // The current route's selected leaf is "Runtime Status".
    const selected = tree.getByRole('treeitem', { selected: true })
    await expect(selected).toHaveText(i18nRegex('sidebar.policies'))
  })
})
