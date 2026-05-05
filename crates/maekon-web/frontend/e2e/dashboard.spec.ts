import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const dashboardHeadingName = i18nRegex('dashboard.title', ['Dashboard preparing', '대시보드 준비 중'])
const activeTimeName = i18nRegex('dashboard.activeTime')
const idleTimeName = i18nRegex('dashboard.idleTime')
const capturesName = i18nRegex('dashboard.captures')
const eventsName = i18nRegex('dashboard.events')
const connectionStatusName = i18nRegex([
  'dashboard.connecting',
  'dashboard.connected',
  'dashboard.disconnected',
  'dashboard.error',
])
const cpuMemorySectionName = i18nRegex('dashboard.cpuMemory24h')
const appUsageSectionName = i18nRegex('dashboard.appUsageTime')
const activityHeatmapName = i18nRegex('dashboard.activityHeatmap')
const systemStatusName = i18nRegex('dashboard.systemStatus')

const mockedSummary = {
  date: '2026-02-23',
  total_active_secs: 7200,
  total_idle_secs: 900,
  top_apps: [
    {
      name: 'Code',
      duration_secs: 3600,
      event_count: 12,
      frame_count: 8,
    },
  ],
  cpu_avg: 21.5,
  memory_avg_percent: 42.7,
  frames_captured: 8,
  events_logged: 16,
}

const mockedHourlyMetrics = [
  {
    hour: '10:00',
    cpu_avg: 21.5,
    cpu_max: 30.1,
    memory_avg: 42.7,
    memory_max: 50.2,
    sample_count: 6,
  },
]

const mockedProcesses = [
  {
    timestamp: '2026-02-23T10:00:00Z',
    processes: [
      {
        pid: 12345,
        name: 'Code',
        cpu_usage: 11.1,
        memory_bytes: 345678912,
      },
    ],
  },
]

const mockedHeatmap = {
  from_date: '2026-02-17',
  to_date: '2026-02-23',
  cells: [
    { day: 1, hour: 10, value: 1 },
    { day: 2, hour: 14, value: 2 },
  ],
  max_value: 2,
}

const mockedFocusMetrics = {
  today: {
    date: '2026-02-23',
    total_active_secs: 7200,
    deep_work_secs: 3600,
    communication_secs: 900,
    context_switches: 5,
    interruption_count: 2,
    avg_focus_duration_secs: 1200,
    max_focus_duration_secs: 2400,
    focus_score: 82,
  },
  history: [],
}

const mockedUpdateStatus = {
  enabled: true,
  auto_install: false,
  phase: 'Idle',
  message: null,
  pending: null,
  revision: 1,
  updated_at: '2026-02-23T10:00:00Z',
}

function dashboardHeading(page: Page) {
  return page.getByRole('heading', { name: dashboardHeadingName })
}

async function mockDashboardApis(page: Page) {
  await mockStaticJson(page, '**/api/stats/summary**', mockedSummary)
  await mockStaticJson(page, '**/api/metrics/hourly**', mockedHourlyMetrics)
  await mockStaticJson(page, '**/api/processes**', mockedProcesses)
  await mockStaticJson(page, '**/api/stats/heatmap**', mockedHeatmap)
  await mockStaticJson(page, '**/api/focus/metrics**', mockedFocusMetrics)
  await mockStaticJson(page, '**/api/focus/suggestions**', [])
  await mockStaticJson(page, '**/api/update/status**', mockedUpdateStatus)
}

test.describe('Dashboard', () => {
  // Sub-route ownership after the Dashboard refactor:
  //   /overview   (default) → OverviewSection (metric cards, connection status)
  //   /monitoring           → MonitoringSection (CPU/Memory chart, app usage)
  //   /insights             → InsightsSection (activity heatmap, system status)
  test.beforeEach(async ({ page }) => {
    await mockDashboardApis(page)
  })

  test('should display dashboard title', async ({ page }) => {
    await page.goto('/overview')
    await expect(dashboardHeading(page)).toBeVisible({ timeout: 10000 })
  })

  test('should display stat cards', async ({ page }) => {
    await page.goto('/overview')
    await expect(page.getByTestId('metric-card-active-time')).toBeVisible()
    await expect(page.getByTestId('metric-card-idle-time')).toBeVisible()
    await expect(page.getByTestId('metric-card-captures')).toBeVisible()
    await expect(page.getByTestId('metric-card-events')).toBeVisible()
  })

  test('should display realtime monitoring section', async ({ page }) => {
    await page.goto('/overview')
    await expect(page.getByText(connectionStatusName).first()).toBeVisible()
  })

  test('should display CPU/Memory chart section', async ({ page }) => {
    await page.goto('/monitoring')
    await expect(page.getByText(cpuMemorySectionName)).toBeVisible()
  })

  test('should display app usage section', async ({ page }) => {
    await page.goto('/monitoring')
    await expect(page.getByText(appUsageSectionName)).toBeVisible()
  })

  test('should display activity heatmap', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.getByRole('heading', { name: activityHeatmapName })).toBeVisible()
  })

  test('should display system status section', async ({ page }) => {
    await page.goto('/insights')
    await expect(page.getByText(systemStatusName)).toBeVisible()
  })

  test('should show connection status indicator', async ({ page }) => {
    await page.goto('/overview')
    const connectionStatus = page.getByText(connectionStatusName)
    await expect(connectionStatus.first()).toBeVisible({ timeout: 10000 })
  })
})
