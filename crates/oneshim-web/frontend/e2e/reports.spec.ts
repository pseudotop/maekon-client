import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const reportsTitleName = i18nRegex('reports.title')
const weekPeriodName = i18nRegex('reports.week')
const monthPeriodName = i18nRegex('reports.month')
const customPeriodName = i18nRegex('reports.custom')
const generateButtonName = i18nRegex('reports.generate')
const productivityScoreName = i18nRegex('reports.productivityScore')
const activeTimeName = i18nRegex('reports.activeTime')
const dailyActivityName = i18nRegex('reports.dailyActivity')
const appUsageName = i18nRegex('reports.appUsage')
const trendName = i18nRegex('reports.trend')
const hourlyActivityName = i18nRegex('reports.hourlyActivity')
const systemMetricsName = i18nRegex('reports.systemMetrics')

const mockedReport = {
  title: 'Test Report',
  from_date: '2026-02-17',
  to_date: '2026-02-23',
  days: 7,
  total_active_secs: 24600,
  total_idle_secs: 3600,
  total_captures: 42,
  total_events: 89,
  avg_cpu: 24.2,
  avg_memory: 55.7,
  daily_stats: [
    {
      date: '2026-02-22',
      active_secs: 3600,
      idle_secs: 900,
      captures: 8,
      events: 16,
      cpu_avg: 21.1,
      memory_avg: 50.2,
    },
    {
      date: '2026-02-23',
      active_secs: 4200,
      idle_secs: 600,
      captures: 10,
      events: 18,
      cpu_avg: 25.7,
      memory_avg: 57.5,
    },
  ],
  app_stats: [
    {
      name: 'Code',
      duration_secs: 7200,
      events: 30,
      captures: 15,
      percentage: 52.1,
    },
    {
      name: 'Chrome',
      duration_secs: 4200,
      events: 21,
      captures: 11,
      percentage: 30.4,
    },
  ],
  hourly_activity: [
    { hour: 9, activity: 22 },
    { hour: 10, activity: 31 },
    { hour: 11, activity: 29 },
  ],
  productivity: {
    score: 82,
    active_ratio: 87.3,
    peak_hour: 10,
    top_app: 'Code',
    trend: 7.2,
  },
}

async function mockReportsApis(page: Page) {
  await mockStaticJson(page, '**/api/reports**', mockedReport)
}

test.describe('Reports', () => {
  // /reports splits into three sub-routes (defaultChild=activity); the period
  // selector lives in ReportsLayout so it appears on every sub-route.
  //   /reports/activity → ActivityReport (daily/hourly activity, app usage)
  //   /reports/focus    → FocusReport (productivity score, summary, trend)
  //   /reports/export   → ExportSection (system metrics)
  test.beforeEach(async ({ page }) => {
    await mockReportsApis(page)
  })

  test('should display reports title', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.getByRole('heading', { name: reportsTitleName })).toBeVisible({ timeout: 10000 })
  })

  test('should display period selector', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.getByRole('button', { name: weekPeriodName })).toBeVisible()
    await expect(page.getByRole('button', { name: monthPeriodName })).toBeVisible()
    await expect(page.getByRole('button', { name: customPeriodName })).toBeVisible()
  })

  test('should display productivity score', async ({ page }) => {
    await page.goto('/reports/focus')
    await expect(page.getByText(productivityScoreName)).toBeVisible()
  })

  test('should display summary statistics', async ({ page }) => {
    await page.goto('/reports/focus')
    await expect(page.getByText(activeTimeName)).toBeVisible()
  })

  test('should display daily activity chart section', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.getByText(dailyActivityName)).toBeVisible()
  })

  test('should display app usage section', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.getByText(appUsageName)).toBeVisible()
  })

  test('should switch period', async ({ page }) => {
    await page.goto('/reports/focus')
    const monthButton = page.getByRole('button', { name: monthPeriodName })
    await monthButton.click()
    await expect(page.getByText(productivityScoreName)).toBeVisible()
  })

  test('should display trend indicator', async ({ page }) => {
    await page.goto('/reports/focus')
    await expect(page.locator('p').filter({ hasText: trendName }).first()).toBeVisible()
    await expect(page.getByText(/[↑↓→]/).first()).toBeVisible()
  })

  test('should display hourly activity section', async ({ page }) => {
    await page.goto('/reports/activity')
    await expect(page.getByText(hourlyActivityName)).toBeVisible()
  })

  test('should display system metrics section', async ({ page }) => {
    await page.goto('/reports/export')
    await expect(page.getByText(systemMetricsName)).toBeVisible()
  })

  test('should select custom date range', async ({ page }) => {
    await page.goto('/reports/focus')
    await page.getByRole('button', { name: customPeriodName }).click()

    const dateInputs = page.locator('input[type="date"]')
    await dateInputs.first().fill('2026-02-01')
    await dateInputs.nth(1).fill('2026-02-07')

    await page.getByRole('button', { name: generateButtonName }).click()
    await expect(page.getByText(productivityScoreName)).toBeVisible()
  })
})
