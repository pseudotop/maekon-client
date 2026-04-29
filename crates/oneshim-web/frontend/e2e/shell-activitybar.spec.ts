import { expect, test } from './helpers/test'

test.describe('ActivityBar Actions', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
  })

  // After the category restructure the ActivityBar only exposes 3 group icons
  // (monitor/data/manage) for the main area and 2 direct icons (settings/
  // privacy) at the bottom.  Clicking a group icon navigates to that group's
  // default path; from there sub-routes are reached through the SidePanel
  // tree, not from the ActivityBar.
  const GROUP_BUTTONS = [
    { id: 'nav-group-monitor', expectedUrl: /\/overview$/ },
    { id: 'nav-group-insights', expectedUrl: /\/reports\/activity$/ },
    { id: 'nav-group-manage', expectedUrl: /\/automation\/policies$/ },
  ]

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
    await page.getByTestId('nav-group-monitor').click()
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

  test('P013: manage group highlights when on a manage route', async ({ page }) => {
    // Navigate to /automation/policies — this should activate the manage
    // group icon (automation moved from monitor to manage group).
    await page.goto('/automation/policies')
    const active = page.locator('nav button[aria-current="page"]')
    await expect(active).toHaveCount(1)
    await expect(active).toHaveAttribute('data-testid', 'nav-group-manage')
  })

  test('P014: activity bar exposes exactly five nav buttons', async ({ page }) => {
    // 3 group icons + 2 bottom direct icons = 5 total.  Guards against
    // accidental re-introduction of per-route icons in the 48px rail.
    const nav = page.locator('nav[aria-label]')
    const buttons = nav.locator('button[data-testid]')
    await expect(buttons).toHaveCount(5)
  })

  test('P015: clicking the active group toggles the SidePanel', async ({ page }) => {
    await page.goto('/overview')
    // Sidepanel visible initially (on /overview which is in monitor group).
    const sidePanel = page.locator('[role="tree"]')
    await expect(sidePanel).toBeVisible()

    // Click the already-active monitor icon → sidepanel should collapse.
    await page.getByTestId('nav-group-monitor').click()
    await expect(sidePanel).toHaveCount(0)

    // Click again → sidepanel re-opens.
    await page.getByTestId('nav-group-monitor').click()
    await expect(sidePanel).toBeVisible()
  })

  test('P016: SidePanel shows the full group tree with nested children', async ({ page }) => {
    await page.goto('/automation/policies')
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()

    // Top-level treeitems correspond to each manage route.  Auto-expand
    // means the nested /automation children are also reachable — but their
    // labels come from i18n (sidebar.policies → "Runtime Status",
    // sidebar.executionHistory → "Execution History"), NOT from the route
    // path segments.
    await expect(tree.getByRole('treeitem', { name: /automation/i })).toBeVisible()
    await expect(tree.getByRole('treeitem', { name: /audit/i })).toBeVisible()
    await expect(tree.getByRole('treeitem', { name: /runtime status/i })).toBeVisible()

    // The current route's selected leaf is "Runtime Status" (the i18n label
    // for the automation.policies tab, not "Policies").
    const selected = tree.getByRole('treeitem', { selected: true })
    await expect(selected).toHaveText(/runtime status/i)
  })
})
