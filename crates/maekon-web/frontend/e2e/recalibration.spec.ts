import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const recalibrationTitleName = i18nRegex('recalibration.title')
const reclusterButtonName = i18nRegex('recalibration.triggerRecluster')
const markRangePersonalName = i18nRegex('recalibration.markRangePersonal')
const overrideHistoryName = i18nRegex('recalibration.overrideHistory')
const noOverridesName = i18nRegex('recalibration.noOverrides')
const noSegmentsName = i18nRegex('recalibration.noSegments')

async function mockRecalibrationApis(page: Page) {
  await mockStaticJson(page, '**/api/recalibration/overrides**', [])
  await mockStaticJson(page, '**/api/dashboard/day**', {
    date: '2026-02-23',
    insight: null,
    timeline: [],
    statistics: {
      deep_work_hours: 0,
      communication_hours: 0,
      meeting_hours: 0,
      context_switches: 0,
      longest_focus_mins: 0,
      longest_focus_content: '',
      regime_distribution: {},
    },
  })
  await mockStaticJson(page, '**/api/recalibration/recluster**', { ok: true, message: 'done' })
}

test.describe('Recalibration', () => {
  test.beforeEach(async ({ page }) => {
    await mockRecalibrationApis(page)
    await page.goto('/recalibration')
    await expect(page.getByRole('heading', { name: recalibrationTitleName })).toBeVisible({ timeout: 10000 })
  })

  test('should display recalibration page title', async ({ page }) => {
    await expect(page.getByRole('heading', { name: recalibrationTitleName })).toBeVisible()
  })

  test('should display trigger re-cluster button', async ({ page }) => {
    await expect(page.getByRole('button', { name: reclusterButtonName })).toBeVisible()
    await expect(page.getByRole('button', { name: reclusterButtonName })).toBeEnabled()
  })

  test('should display mark range as personal time button', async ({ page }) => {
    await expect(page.getByRole('button', { name: markRangePersonalName })).toBeVisible()
  })

  test('should display override history section with empty state', async ({ page }) => {
    // Override history lives in OverridesSection at /recalibration/overrides
    // (the default sub-route is /recalibration/segments).
    await page.goto('/recalibration/overrides')
    await expect(page.getByRole('heading', { name: overrideHistoryName })).toBeVisible()
    await expect(page.getByText(noOverridesName)).toBeVisible()
  })

  test('should display empty segments state', async ({ page }) => {
    await expect(page.getByText(noSegmentsName)).toBeVisible()
  })
})
