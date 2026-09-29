import type { DailyDigestResponse } from '../src/api/contracts'
import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const dailyTimetableName = i18nRegex('dashboard.dailyTimetable')
const previousDayName = i18nRegex('dashboard.previousDay')
const nextDayName = i18nRegex('dashboard.nextDay')
const pomodoroTitleName = i18nRegex('focus.pomodoro.title')
const heuristicDigestName = new RegExp(`^(?:${i18nRegex('summaryProvenance.digest.heuristic').source})$`, 'i')
const aiNarrativeBadge = new RegExp(`^(?:${i18nRegex('summaryProvenance.aiDailyNarrative').source}) · `, 'i')

const aiNarrative = 'You spent most of your morning in deep work on VS Code.'
const legacyNarrative = 'Legacy daily narrative without AI provenance.'
const generatedAt = '2026-02-23T12:00:00Z'

const mockedDigest: DailyDigestResponse = {
  date: '2026-02-23',
  generated_at: generatedAt,
  digest_provenance: 'heuristic',
  ai_narrative: {
    text: aiNarrative,
    provider_class: 'loopback',
    generated_at: generatedAt,
  },
  insight: {
    narrative: legacyNarrative,
    highlights: [{ text: '2h 15m uninterrupted', highlight_type: 'ACHIEVEMENT' }],
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

  test('should display the AI artifact narrative with provenance', async ({ page }) => {
    await expect(page.getByText(aiNarrative, { exact: true })).toBeVisible()
    await expect(page.getByText(aiNarrativeBadge)).toBeVisible()
    await expect(page.getByText(aiNarrativeBadge)).toContainText(i18nRegex('summaryProvenance.provider.loopback'))
    await expect(page.getByText(heuristicDigestName)).toBeVisible()
    await expect(page.locator(`time[datetime="${generatedAt}"]`)).toBeVisible()
    await expect(page.getByText('2h 15m uninterrupted')).toBeVisible()
    await expect(page.getByText(legacyNarrative, { exact: true })).toHaveCount(0)
    await expect(page.getByText(i18nRegex('summaryProvenance.dailyNarrativeUnavailable'))).toHaveCount(0)
  })

  for (const scenario of [
    { name: 'legacy response without an artifact', artifact: undefined, reason: 'not_generated' },
    { name: 'failed AI generation', artifact: { failure_reason: 'provider_failed' }, reason: 'provider_failed' },
  ] as const) {
    test(`should not present a ${scenario.name} as an AI narrative`, async ({ page }) => {
      // Undefined intentionally models an older wire response missing the artifact.
      await mockStaticJson(page, '**/api/dashboard/day**', { ...mockedDigest, ai_narrative: scenario.artifact })
      await page.reload()

      await expect(page.getByText(i18nRegex('summaryProvenance.dailyNarrativeUnavailable'))).toBeVisible()
      await expect(page.getByText(i18nRegex(`summaryProvenance.failure.${scenario.reason}`))).toBeVisible()
      await expect(page.getByText(heuristicDigestName)).toBeVisible()
      await expect(page.getByText(aiNarrativeBadge)).toHaveCount(0)
      await expect(page.getByText(aiNarrative, { exact: true })).toHaveCount(0)
      await expect(page.getByText(legacyNarrative, { exact: true })).toHaveCount(0)
    })
  }

  test('should display pomodoro timer sidebar', async ({ page }) => {
    await expect(page.getByText(pomodoroTitleName)).toBeVisible()
  })

  test('should disable next-day button when viewing today', async ({ page }) => {
    await expect(page.getByRole('button', { name: nextDayName })).toBeDisabled()
  })
})
