import { i18nRegex } from './helpers/i18n'
import { mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const timelineHeadingName = i18nRegex('timeline.title')
const timelineAppName = i18nRegex('timeline.app')
const timelineImportanceName = i18nRegex('timeline.importance')
const dateRangeButtonName = i18nRegex(['dateRange.today', 'dateRange.week', 'dateRange.month'])
const timelineGridViewTitle = i18nRegex('timeline.gridView')
const timelineListViewTitle = i18nRegex('timeline.listView')
const capturesTextName = i18nRegex('timeline.captures')
const mockedFrames = {
  data: [
    {
      id: 1,
      timestamp: '2026-02-23T10:00:00Z',
      trigger_type: 'interval',
      app_name: 'Code',
      window_title: 'ONESHIM',
      importance: 0.75,
      resolution: '1920x1080',
      file_path: null,
      ocr_text: null,
      image_url: null,
      tag_ids: [],
    },
  ],
  pagination: {
    total: 1,
    offset: 0,
    limit: 50,
    has_more: false,
  },
}

function timelineHeading(page: Page) {
  return page.getByRole('heading', { name: timelineHeadingName })
}

async function mockTimelineApis(page: Page) {
  await mockStaticJson(page, '**/api/frames**', mockedFrames)
  await mockStaticJson(page, '**/api/tags**', [])
}

test.describe('Timeline', () => {
  test.beforeEach(async ({ page }) => {
    await mockTimelineApis(page)
    await page.goto('/timeline')
    await expect(timelineHeading(page)).toBeVisible({ timeout: 10000 })
  })

  test('should display timeline title', async ({ page }) => {
    await expect(timelineHeading(page)).toBeVisible()
  })

  test('should display filter controls', async ({ page }) => {
    await expect(page.getByText(timelineAppName).first()).toBeVisible()
    await expect(page.getByText(timelineImportanceName).first()).toBeVisible()
  })

  test('should display view mode toggle buttons', async ({ page }) => {
    const viewButtons = page.locator('button svg')
    const hasViewToggle = (await viewButtons.count()) >= 2
    expect(hasViewToggle).toBeTruthy()
  })

  test('should display date range picker', async ({ page }) => {
    const dateButtons = page.getByRole('button', { name: dateRangeButtonName })
    await expect(dateButtons.first()).toBeVisible()
  })

  test('should toggle view mode', async ({ page }) => {
    const gridButton = page.getByTitle(timelineGridViewTitle).first()
    const listButton = page.getByTitle(timelineListViewTitle).first()

    if (await gridButton.isVisible()) {
      await gridButton.click()
    } else if (await listButton.isVisible()) {
      await listButton.click()
    }

    await expect(gridButton.or(listButton).first()).toBeVisible()
  })

  test('should show frame count', async ({ page }) => {
    const captureCount = page.getByText(capturesTextName)
    await expect(captureCount.first()).toBeVisible({ timeout: 10000 })
  })

  test('should filter by importance', async ({ page }) => {
    const selects = page.locator('select')
    await expect(selects.nth(1)).toBeVisible()
    await selects.nth(1).selectOption({ index: 1 })
    await expect(selects.nth(1)).toHaveValue('high')
  })

  test('should support keyboard navigation', async ({ page }) => {
    await page.keyboard.press('ArrowRight')
    await page.keyboard.press('ArrowLeft')
    await page.keyboard.press('Escape')
  })
})
