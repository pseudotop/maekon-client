import { i18nRegex, i18nTextIn, type ShippedLocale } from './helpers/i18n'
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
// 1.5 hours of deep work in whichever language the duration is written.
const deepWorkValue = /^1\D+30\D*$/

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

// #13735: the rule-based digest the overview's one-line summary and key
// numbers come from. No AI narrative, as for a profile without a provider.
const mockedDigest = {
  date: '2026-02-23',
  insight: null,
  timeline: [
    {
      segment_id: 'seg-1',
      start_time: '2026-02-23T09:00:00Z',
      end_time: '2026-02-23T09:48:00Z',
      duration_mins: 48,
      regime_label: 'Deep Work',
      regime_color: '#14b8a6',
      dominant_app: 'Code',
      content_summary: [],
    },
  ],
  statistics: {
    deep_work_hours: 1.5,
    communication_hours: 0.25,
    meeting_hours: 0,
    context_switches: 12,
    longest_focus_mins: 48,
    longest_focus_content: '',
    regime_distribution: {},
  },
  generated_at: '2026-02-23T10:00:00Z',
  digest_provenance: 'heuristic',
  ai_narrative: { failure_reason: 'not_generated' },
}

// Large enough to exercise the widest value the cards show ("12h 59m").
const longDaySummary = {
  ...mockedSummary,
  total_active_secs: 12 * 3600 + 59 * 60,
  total_idle_secs: 10 * 3600 + 45 * 60,
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
  await mockStaticJson(page, '**/api/dashboard/day**', mockedDigest)
  await mockStaticJson(page, '**/api/metrics/hourly**', mockedHourlyMetrics)
  await mockStaticJson(page, '**/api/processes**', mockedProcesses)
  await mockStaticJson(page, '**/api/stats/heatmap**', mockedHeatmap)
  await mockStaticJson(page, '**/api/focus/metrics**', mockedFocusMetrics)
  await mockStaticJson(page, '**/api/focus/suggestions**', [])
  await mockStaticJson(page, '**/api/update/status**', mockedUpdateStatus)
}

test.describe('Dashboard', () => {
  // Route ownership after #13735:
  //   /overview   (default) → OverviewSection (today's summary, next up, key numbers)
  //   /monitoring           → MonitoringSection, its own page now (CPU/Memory chart,
  //                           app usage, capture counts), linked from /support
  //   /insights             → InsightsSection (activity heatmap, system status)
  test.beforeEach(async ({ page }) => {
    await mockDashboardApis(page)
  })

  test('should display dashboard title', async ({ page }) => {
    await page.goto('/overview')
    await expect(dashboardHeading(page)).toBeVisible({ timeout: 10000 })
  })

  test('should display the key numbers on the first screen', async ({ page }) => {
    await page.goto('/overview')
    await expect(page.getByTestId('metric-card-active-time')).toBeVisible()
    await expect(page.getByTestId('metric-card-deep-work-value')).toHaveText(deepWorkValue)
    await expect(page.getByTestId('metric-card-context-switches-value')).toHaveText('12')
    await expect(page.getByTestId('metric-card-idle-time')).toBeVisible()
    // Capture and event counts moved to System Metrics with the CPU data.
    await expect(page.getByTestId('metric-card-captures')).toHaveCount(0)
    await expect(page.getByTestId('metric-card-events')).toHaveCount(0)
  })

  test('should open with the rule-based one-line summary and its provenance', async ({ page }) => {
    await page.goto('/overview')
    await expect(page.getByTestId('today-summary-headline')).toContainText('Code')
    await expect(page.getByTestId('today-summary-provenance')).toHaveAttribute('data-provenance', 'heuristic')
    // Order of the first screen: summary, then next up, then the numbers.
    const top = async (testId: string) => (await page.getByTestId(testId).boundingBox())?.y ?? Number.NaN
    const [summaryTop, nextTop, numbersTop] = [
      await top('today-summary'),
      await top('next-actions'),
      await top('overview-key-numbers'),
    ]
    expect(summaryTop).toBeLessThan(nextTop)
    expect(nextTop).toBeLessThan(numbersTop)
  })

  test('should keep capture counts on the System Metrics page', async ({ page }) => {
    await page.goto('/monitoring')
    await expect(page.getByTestId('metric-card-captures')).toBeVisible()
    await expect(page.getByTestId('metric-card-events')).toBeVisible()
  })

  test('should reach System Metrics from Support', async ({ page }) => {
    await page.goto('/support')
    await page.getByTestId('support-system-metrics').getByRole('button').click()
    await expect(page).toHaveURL(/\/monitoring$/)
    await expect(page.locator('#section-metrics')).toBeVisible()
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

// #13735: at the minimum window width (800px, tauri.conf.json) and at 900px,
// a key-number value or label must stay on one line and inside its card, in
// every shipped locale.
// #13749: each case first checks that the label on screen is that locale's own
// text. zh-CN used to render the English bundle, so its cases measured English
// and passed.
test.describe('Dashboard key numbers at narrow window widths', () => {
  const shippedLocales: ShippedLocale[] = ['en', 'ko', 'ja', 'zh-CN', 'es']
  const activeTimeLabelIn = (language: ShippedLocale) => i18nTextIn(language, 'dashboard.activeTime')

  test('uses a label that reads differently in every shipped locale', () => {
    expect(new Set(shippedLocales.map(activeTimeLabelIn)).size).toBe(shippedLocales.length)
  })

  for (const width of [800, 900]) {
    for (const language of shippedLocales) {
      test(`stay on one line at ${width}px (${language})`, async ({ page }) => {
        await page.addInitScript((code) => window.localStorage.setItem('maekon-language', code), language)
        await page.setViewportSize({ width, height: 900 })
        await mockDashboardApis(page)
        await mockStaticJson(page, '**/api/stats/summary**', longDaySummary)
        await page.goto('/overview')
        await expect(page.getByTestId('metric-card-deep-work-value')).toHaveText(deepWorkValue)
        await expect(page.getByTestId('metric-card-active-time-label')).toHaveText(activeTimeLabelIn(language))

        const texts = page
          .getByTestId('overview-key-numbers')
          .locator('[data-testid$="-value"], [data-testid$="-label"]')
        await expect(texts).toHaveCount(8)
        const measured = await texts.evaluateAll((elements) =>
          elements.map((element) => {
            const id = element.getAttribute('data-testid') ?? ''
            // Measure the rendered text against the card's content box, not the
            // element's own box: a flex item that refuses to shrink overflows
            // its card without overflowing itself.
            const card = document.querySelector(`[data-testid="${id.replace(/-(value|label)$/, '')}"]`)
            if (!card) return { id, text: element.textContent, lines: 0, overflow: Number.POSITIVE_INFINITY }
            const range = document.createRange()
            range.selectNodeContents(element)
            const rects = [...range.getClientRects()]
            const box = card.getBoundingClientRect()
            const style = getComputedStyle(card)
            const innerLeft = box.left + Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.paddingLeft)
            const innerRight =
              box.right - Number.parseFloat(style.borderRightWidth) - Number.parseFloat(style.paddingRight)
            return {
              id,
              text: element.textContent,
              lines: new Set(rects.map((rect) => Math.round(rect.top))).size,
              overflow: Math.max(...rects.map((rect) => Math.max(rect.right - innerRight, innerLeft - rect.left))),
            }
          }),
        )
        for (const text of measured) {
          expect(text.lines, `${text.id} "${text.text}" wraps`).toBe(1)
          // Half a pixel of slack for subpixel text positioning.
          expect(text.overflow, `${text.id} "${text.text}" overflows its card`).toBeLessThanOrEqual(0.5)
        }
      })
    }
  }
})
