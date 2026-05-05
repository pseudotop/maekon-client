import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const coachingTitleName = i18nRegex('coaching.title')
const goalsTitleName = i18nRegex('coaching.goalsTitle')
const recentEventsName = i18nRegex('coaching.recentEvents')
const noGoalsName = i18nRegex('coaching.noGoals')
const noEventsName = i18nRegex('coaching.noEvents')

const mockedGoals = [
  {
    regime_label: 'Deep Work',
    current_minutes: 90,
    target_minutes: 120,
    percentage: 75,
    display_color: '#14b8a6',
  },
  {
    regime_label: 'Communication',
    current_minutes: 30,
    target_minutes: 60,
    percentage: 50,
    display_color: '#f59e0b',
  },
]

const mockedHistory = [
  {
    event_id: 'evt-1',
    trigger_type: 'regime_drift',
    profile_name: 'FocusGuard',
    regime_id: 'deep-work',
    message_template: 'You have been focused for 45 minutes.',
    personalized_message: 'Great focus session! Consider a short break.',
    shown_at: '2026-02-23T10:00:00Z',
    dismissed_at: '2026-02-23T10:01:00Z',
    dismiss_action: 'snoozed',
    feedback_type: 'helpful',
    feedback_score: 1,
  },
  {
    event_id: 'evt-2',
    trigger_type: 'context_switch',
    profile_name: 'TimeAware',
    regime_id: null,
    message_template: 'Frequent context switches detected.',
    personalized_message: null,
    shown_at: '2026-02-23T09:30:00Z',
    dismissed_at: null,
    dismiss_action: null,
    feedback_type: null,
    feedback_score: null,
  },
]

async function mockCoachingApis(page: Page, opts?: { emptyGoals?: boolean; emptyHistory?: boolean }) {
  await mockStaticJson(page, '**/api/coaching/goals**', opts?.emptyGoals ? [] : mockedGoals)
  await mockStaticJson(page, '**/api/coaching/history**', opts?.emptyHistory ? [] : mockedHistory)
}

test.describe('Coaching', () => {
  // /coaching now splits into two sub-routes (defaultChild=goals):
  //   /coaching/goals   → GoalsSection (goal progress cards)
  //   /coaching/history → HistorySection (recent events list)
  test.beforeEach(async ({ page }) => {
    await mockCoachingApis(page)
  })

  test('should display coaching page title', async ({ page }) => {
    await page.goto('/coaching/goals')
    await expect(page.getByRole('heading', { name: coachingTitleName })).toBeVisible({ timeout: 10000 })
  })

  test('should display goal progress section', async ({ page }) => {
    await page.goto('/coaching/goals')
    await expect(page.getByRole('heading', { name: goalsTitleName })).toBeVisible()
    await expect(page.getByText('Deep Work')).toBeVisible()
    await expect(page.getByText('Communication')).toBeVisible()
    await expect(page.getByText('75%')).toBeVisible()
  })

  test('should display coaching events section', async ({ page }) => {
    await page.goto('/coaching/history')
    await expect(page.getByRole('heading', { name: recentEventsName })).toBeVisible()
    await expect(page.getByText('FocusGuard')).toBeVisible()
    await expect(page.getByText('TimeAware')).toBeVisible()
  })

  test('should display event messages', async ({ page }) => {
    await page.goto('/coaching/history')
    await expect(page.getByText('Great focus session! Consider a short break.')).toBeVisible()
    await expect(page.getByText('Frequent context switches detected.')).toBeVisible()
  })

  test('should display feedback indicator on events that have feedback', async ({ page }) => {
    await page.goto('/coaching/history')
    await expect(page.getByText('helpful')).toBeVisible()
  })
})

test.describe('Coaching — empty state', () => {
  test('should show empty state when no goals or events', async ({ page }) => {
    await mockCoachingApis(page, { emptyGoals: true, emptyHistory: true })

    await page.goto('/coaching/goals')
    await expect(page.getByText(noGoalsName)).toBeVisible({ timeout: 10000 })

    await page.goto('/coaching/history')
    await expect(page.getByText(noEventsName)).toBeVisible({ timeout: 10000 })
  })
})
