import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const dailyTimetableName = i18nRegex('dashboard.dailyTimetable')
const previousDayName = i18nRegex('dashboard.previousDay')
const nextDayName = i18nRegex('dashboard.nextDay')
const pomodoroTitleName = i18nRegex('focus.pomodoro.title')

const mockedDigest = {
  date: '2026-02-23',
  insight: {
    narrative: 'You spent most of your morning in deep work on VS Code.',
    highlights: [{ label: 'Deep Focus', detail: '2h 15m uninterrupted', highlight_type: 'positive' }],
  },
  timeline: [
    {
      segment_id: 'seg-1',
      start_time: '2026-02-23T09:00:00Z',
      end_time: '2026-02-23T11:15:00Z',
      duration_mins: 135,
      regime_label: 'Deep Work',
      regime_color: '#14b8a6',
      regime_id: 'deep-work',
      dominant_app: 'VS Code',
      content_summary: [{ content: 'Coding on project', work_type: 'development', mins: 135 }],
    },
    {
      segment_id: 'seg-2',
      start_time: '2026-02-23T11:15:00Z',
      end_time: '2026-02-23T11:45:00Z',
      duration_mins: 30,
      regime_label: 'Communication',
      regime_color: '#f59e0b',
      regime_id: 'communication',
      dominant_app: 'Slack',
      content_summary: [{ content: 'Team standup', work_type: 'communication', mins: 30 }],
    },
  ],
  statistics: {
    deep_work_hours: 2.25,
    communication_hours: 0.5,
    meeting_hours: 0,
    context_switches: 3,
    longest_focus_mins: 135,
    longest_focus_content: 'VS Code - project work',
    regime_distribution: { 'Deep Work': 75, Communication: 25 },
  },
}

async function mockDashboardDayApis(page: Page) {
  await mockStaticJson(page, '**/api/dashboard/day**', mockedDigest)
  await mockStaticJson(page, '**/api/recalibration/overrides**', [])
  await mockStaticJson(page, '**/api/stats/gui-heatmap**', [])
  await mockStaticJson(page, '**/api/pomodoro/current**', null)
}

test.describe('Dashboard Day', () => {
  test.beforeEach(async ({ page }) => {
    await mockDashboardDayApis(page)
    await page.goto('/day')
    await expect(page.getByRole('heading', { name: dailyTimetableName })).toBeVisible({ timeout: 10000 })
  })

  test('should display daily timetable heading', async ({ page }) => {
    await expect(page.getByRole('heading', { name: dailyTimetableName })).toBeVisible()
  })

  test('should display date navigation controls', async ({ page }) => {
    await expect(page.getByRole('button', { name: previousDayName })).toBeVisible()
    await expect(page.getByRole('button', { name: nextDayName })).toBeVisible()
    await expect(page.locator('input[type="date"]')).toBeVisible()
  })

  test('should display insight narrative', async ({ page }) => {
    await expect(
      page.getByText('You spent most of your morning in deep work on VS Code.'),
    ).toBeVisible()
  })

  test('should display pomodoro timer sidebar', async ({ page }) => {
    await expect(page.getByText(pomodoroTitleName)).toBeVisible()
  })

  test('should disable next-day button when viewing today', async ({ page }) => {
    await expect(page.getByRole('button', { name: nextDayName })).toBeDisabled()
  })
})
