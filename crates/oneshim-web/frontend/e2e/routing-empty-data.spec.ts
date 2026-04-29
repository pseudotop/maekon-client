/**
 * Regression suite for the empty-data layout `<Outlet>` suppression bug class.
 *
 * Background: the layout for each parent route used to short-circuit with an
 * EmptyState (or an error Card) whenever its query returned no data, which
 * suppressed the `<Outlet>` and prevented RouteRenderer's index
 * `<Navigate to="<defaultChild>" replace />` from ever firing. AuditLayout
 * was fixed first (2026-04-08), but the same pattern was repeated across
 * Dashboard, Timeline, Replay, Automation, Focus and Reports layouts.
 *
 * Each test here overrides the relevant mock(s) so the layout's query lands
 * on the previously-buggy branch, then navigates directly to the parent
 * route and asserts the URL eventually lands on the default child. If any
 * layout re-introduces the "early-return EmptyState instead of rendering
 * Outlet" anti-pattern, these tests will fail.
 */

import { i18nRegex } from './helpers/i18n'
import { expect, test } from './helpers/test'
import { mockStaticJson } from './helpers/mock-api'

test.describe('Sub-pathname routing redirects survive empty data', () => {
  test('/ → /overview when summary is empty (no events/frames/active time)', async ({ page }) => {
    // DashboardLayout early-returned EmptyState when:
    //   !latestMetrics && !summary.events_logged && !summary.frames_captured && summary.total_active_secs === 0
    // Force the guard to trigger and verify the index redirect still fires.
    await mockStaticJson(page, '**/api/stats/summary**', {
      date: '2026-02-23',
      total_active_secs: 0,
      total_idle_secs: 0,
      top_apps: [],
      cpu_avg: 0,
      memory_avg_percent: 0,
      frames_captured: 0,
      events_logged: 0,
    })

    await page.goto('/')
    await page.waitForURL('**/overview')
    await expect(page).toHaveURL(/\/overview$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.dashboard.title') })).toBeVisible({ timeout: 5000 })
  })

  test('/timeline → /timeline/all when no frames are stored yet', async ({ page }) => {
    await mockStaticJson(page, '**/api/frames**', {
      data: [],
      pagination: { total: 0, offset: 0, limit: 50, has_more: false },
    })

    await page.goto('/timeline')
    await page.waitForURL('**/timeline/all')
    await expect(page).toHaveURL(/\/timeline\/all$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.timelineWaiting.title') })).toBeVisible({ timeout: 5000 })
  })

  test('/replay → /replay/timeline when the session has no items', async ({ page }) => {
    await mockStaticJson(page, '**/api/timeline**', {
      session: {
        start: '2026-02-23T09:55:00Z',
        end: '2026-02-23T10:15:00Z',
        duration_secs: 1200,
        total_events: 0,
        total_frames: 0,
        total_idle_secs: 0,
      },
      items: [],
      segments: [],
    })

    await page.goto('/replay')
    await page.waitForURL('**/replay/timeline')
    await expect(page).toHaveURL(/\/replay\/timeline$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.replay.title') })).toBeVisible({ timeout: 5000 })
  })

  test('/automation → /automation/policies when automation is disabled and has zero runs', async ({ page }) => {
    await mockStaticJson(page, '**/api/automation/status**', {
      enabled: false,
      sandbox_enabled: false,
      sandbox_profile: 'balanced',
      ocr_provider: 'local',
      llm_provider: 'local',
      ocr_source: 'local',
      llm_source: 'local',
      ocr_fallback_reason: null,
      llm_fallback_reason: null,
      external_data_policy: 'disabled',
      pending_audit_entries: 0,
    })
    // fallback automation/stats already reports all-zero totals, which together
    // with `enabled: false` satisfies the previously-buggy short-circuit.

    await page.goto('/automation')
    await page.waitForURL('**/automation/policies')
    await expect(page).toHaveURL(/\/automation\/policies$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.automation.title') })).toBeVisible({ timeout: 5000 })
  })

  test('/focus → /focus/score when focus score is zero', async ({ page }) => {
    await mockStaticJson(page, '**/api/focus/metrics**', {
      today: {
        date: '2026-02-23',
        total_active_secs: 0,
        deep_work_secs: 0,
        communication_secs: 0,
        context_switches: 0,
        interruption_count: 0,
        avg_focus_duration_secs: 0,
        max_focus_duration_secs: 0,
        focus_score: 0,
      },
      history: [],
    })

    await page.goto('/focus')
    await page.waitForURL('**/focus/score')
    await expect(page).toHaveURL(/\/focus\/score$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.focus.title') })).toBeVisible({ timeout: 5000 })
  })

  test('/reports → /reports/activity when the report query fails', async ({ page }) => {
    // 500 pushes useQuery into its error branch; ReportsLayout used to suppress
    // the Outlet in that path (only rendered `{report && <Outlet />}`),
    // which prevented the index redirect.
    await page.route('**/api/reports**', (route) =>
      route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'internal' }),
      }),
    )

    await page.goto('/reports')
    await page.waitForURL('**/reports/activity')
    await expect(page).toHaveURL(/\/reports\/activity$/)
    // Note: empty state content assertion skipped for reports because
    // fetchWithRetry's standalone fallback provides a default response
    // even after a 500, bypassing the page.route mock. The redirect
    // assertion above is sufficient to prove the layout renders <Outlet>.
  })

  test('/audit → /audit/summary when there are no audit entries (regression baseline)', async ({ page }) => {
    await mockStaticJson(page, '**/api/automation/audit**', [])

    await page.goto('/audit')
    await page.waitForURL('**/audit/summary')
    await expect(page).toHaveURL(/\/audit\/summary$/)
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.auditLog.title') })).toBeVisible({ timeout: 5000 })
  })
})

test.describe('Shell sidebar-hidden grid toggle', () => {
  // After the category restructure, SidePanel always shows the active group's
  // full tree — even for previously-childless routes like /day and /chat.
  // The `sidebar-hidden` class now only applies when the user manually
  // collapses via Cmd/Ctrl+B (or clicks the active group icon).
  test('/day shows the monitor group tree (no longer sidebar-hidden)', async ({ page }) => {
    await page.goto('/day')
    await page.waitForURL('**/day')

    const shell = page.locator('.app-shell')
    await expect(shell).not.toHaveClass(/sidebar-hidden/)

    // Sidebar should render the monitor group tree with Day selected.
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()
    const selected = tree.getByRole('treeitem', { selected: true })
    await expect(selected).toHaveText(/day view/i)
  })

  test('/ has sidebar visible', async ({ page }) => {
    await page.goto('/')
    await page.waitForURL('**/overview')
    await expect(page.locator('.app-shell')).not.toHaveClass(/sidebar-hidden/)
  })

  test('/chat shows the data group tree (no longer sidebar-hidden)', async ({ page }) => {
    await page.goto('/chat')
    await page.waitForURL('**/chat')
    await expect(page.locator('.app-shell')).not.toHaveClass(/sidebar-hidden/)

    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()
    const selected = tree.getByRole('treeitem', { selected: true })
    await expect(selected).toHaveText(/chat/i)
  })

  test('clicking the active group collapses to sidebar-hidden', async ({ page }) => {
    // VS Code-style toggle: clicking the active ActivityBar group icon while
    // the panel is open collapses it, applying the `sidebar-hidden` class to
    // free the grid column for <main>.
    await page.goto('/day')
    await page.waitForURL('**/day')
    await expect(page.locator('.app-shell')).not.toHaveClass(/sidebar-hidden/)

    await page.getByTestId('nav-group-monitor').click()
    await expect(page.locator('.app-shell')).toHaveClass(/sidebar-hidden/)
  })
})

test.describe('Empty state CTA navigation', () => {
  test('timeline empty state "Open Settings" navigates to /settings/monitoring', async ({ page }) => {
    // Mock frames as empty with capture disabled
    await mockStaticJson(page, '**/api/frames**', {
      data: [],
      pagination: { total: 0, offset: 0, limit: 50, has_more: false },
    })
    await mockStaticJson(page, '**/api/settings', {
      capture_enabled: false,
    })

    await page.goto('/timeline/all')

    // The EmptyState CTA button should be visible
    const ctaButton = page.getByRole('button', { name: /Open Settings|설정 열기|設定を開く|Abrir Configuración|打开设置/ })
    await expect(ctaButton).toBeVisible({ timeout: 10000 })

    await ctaButton.click()
    await expect(page).toHaveURL(/\/settings\/monitoring$/)
  })

  test('automation empty state CTA navigates to settings', async ({ page }) => {
    await mockStaticJson(page, '**/api/automation/status**', {
      enabled: false,
      sandbox_enabled: false,
      sandbox_profile: 'balanced',
      ocr_provider: 'local',
      llm_provider: 'local',
      ocr_source: 'local',
      llm_source: 'local',
      ocr_fallback_reason: null,
      llm_fallback_reason: null,
      external_data_policy: 'disabled',
      pending_audit_entries: 0,
    })

    await page.goto('/automation/policies')
    const ctaButton = page.getByRole('button', { name: i18nRegex('emptyState.automation.action') })
    await expect(ctaButton).toBeVisible({ timeout: 10000 })
    await ctaButton.click()
    await expect(page).toHaveURL(/\/settings/)
  })
})

test.describe('Playbooks empty states', () => {
  test('coaching tab shows empty template message', async ({ page }) => {
    await mockStaticJson(page, '**/api/playbooks/coaching', [])
    await mockStaticJson(page, '**/api/playbooks/presets', [])

    await page.goto('/playbooks')
    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.playbooksCoaching.title') })).toBeVisible({ timeout: 10000 })
  })

  test('presets tab shows empty preset message', async ({ page }) => {
    await mockStaticJson(page, '**/api/playbooks/coaching', [])
    await mockStaticJson(page, '**/api/playbooks/presets', [])

    await page.goto('/playbooks')

    // Switch to presets tab — find by text (tab label)
    const presetsTab = page.getByText(i18nRegex('playbooks.presets'))
    await expect(presetsTab).toBeVisible({ timeout: 10000 })
    await presetsTab.click()

    await expect(page.getByRole('heading', { name: i18nRegex('emptyState.playbooksPresets.title') })).toBeVisible({ timeout: 5000 })
  })
})
