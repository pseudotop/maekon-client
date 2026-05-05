import { i18nRegex } from './helpers/i18n'
import { mockDynamicJson, mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const searchTitleName = i18nRegex('search.title')
const searchPlaceholderName = i18nRegex('search.placeholder')
const searchButtonName = i18nRegex('common.search')
const allFilterName = i18nRegex('common.all')
const framesFilterName = i18nRegex('search.frames')
const eventsFilterName = i18nRegex('search.events')
const filterByTagsName = i18nRegex('search.filterByTags')
const resultsLabelName = i18nRegex('search.results')
const searchHintName = i18nRegex('search.searchHint')
const selectedTagsName = i18nRegex('search.selectedTags')
const screenshotBadgeName = i18nRegex('search.screenshot')
const prevPageName = i18nRegex('common.prev')
const nextPageName = i18nRegex('common.next')

const mockedTags = [
  { id: 1, name: 'Focus', color: '#14b8a6', created_at: '2026-02-23T00:00:00Z' },
  { id: 2, name: 'Work', color: '#3b82f6', created_at: '2026-02-23T00:00:00Z' },
]

function buildSearchResult(index: number): {
  result_type: 'frame' | 'event'
  id: string
  timestamp: string
  app_name: string
  window_title: string
  matched_text: string
  image_url: null
  importance: number
  tags: Array<{ id: number; name: string; color: string }>
} {
  const isFrame = index % 2 === 0
  const tag = mockedTags[index % mockedTags.length]

  return {
    result_type: isFrame ? 'frame' : 'event',
    id: String(index + 1),
    timestamp: '2026-02-23T10:00:00Z',
    app_name: isFrame ? 'Code' : 'Chrome',
    window_title: isFrame ? `Editor ${index + 1}` : `Browser ${index + 1}`,
    matched_text: 'test query match',
    image_url: null,
    importance: 0.8,
    tags: [{ id: tag.id, name: tag.name, color: tag.color }],
  }
}

async function mockSearchApis(page: Page) {
  await mockStaticJson(page, '**/api/tags**', mockedTags)

  await mockDynamicJson(page, '**/api/search**', async (request) => {
    const url = new URL(request.url())
    const query = url.searchParams.get('q') ?? ''
    const searchType = url.searchParams.get('search_type') ?? 'all'
    const limit = Number(url.searchParams.get('limit') ?? '20')
    const offset = Number(url.searchParams.get('offset') ?? '0')
    const tagIds = (url.searchParams.get('tag_ids') ?? '')
      .split(',')
      .filter(Boolean)
      .map((tagId) => Number(tagId))

    let rows = Array.from({ length: 25 }, (_, index) => buildSearchResult(index))

    if (searchType === 'frames') {
      rows = rows.filter((row) => row.result_type === 'frame')
    } else if (searchType === 'events') {
      rows = rows.filter((row) => row.result_type === 'event')
    }

    if (tagIds.length > 0) {
      rows = rows.filter((row) => row.tags.some((tag) => tagIds.includes(tag.id)))
    }

    if (query.toLowerCase() === 'none') {
      rows = []
    }

    const paginated = rows.slice(offset, offset + limit)

    return {
      query,
      total: rows.length,
      offset,
      limit,
      results: paginated,
    }
  })
}

test.describe('Search', () => {
  test.beforeEach(async ({ page }) => {
    await mockSearchApis(page)
    await page.goto('/search')
    await expect(page.locator('h1').filter({ hasText: searchTitleName })).toBeVisible({ timeout: 10000 })
  })

  test('should display search title', async ({ page }) => {
    await expect(page.locator('h1').filter({ hasText: searchTitleName })).toBeVisible()
  })

  test('should display search input', async ({ page }) => {
    await expect(page.getByPlaceholder(searchPlaceholderName)).toBeVisible()
  })

  test('should display search type selector', async ({ page }) => {
    await expect(page.getByRole('button', { name: allFilterName })).toBeVisible()
    await expect(page.getByRole('button', { name: framesFilterName })).toBeVisible()
    await expect(page.getByRole('button', { name: eventsFilterName })).toBeVisible()
  })

  test('should display tag filter section', async ({ page }) => {
    await expect(page.getByText(filterByTagsName)).toBeVisible()
  })

  test('should perform search', async ({ page }) => {
    const searchInput = page.getByPlaceholder(searchPlaceholderName)
    await searchInput.fill('test')
    await page.locator('form').getByRole('button', { name: searchButtonName }).click()

    await expect(page.getByText(resultsLabelName)).toBeVisible()
  })

  test('should filter by search type', async ({ page }) => {
    const searchInput = page.getByPlaceholder(searchPlaceholderName)
    await searchInput.fill('test')
    await page.getByRole('button', { name: framesFilterName }).click()
    await page.locator('form').getByRole('button', { name: searchButtonName }).click()

    await expect(page.getByText(screenshotBadgeName).first()).toBeVisible()
  })

  test('should clear search', async ({ page }) => {
    const searchInput = page.getByPlaceholder(searchPlaceholderName)
    await searchInput.fill('test')
    await searchInput.clear()
    await expect(searchInput).toHaveValue('')
  })

  test('should show search hint', async ({ page }) => {
    await expect(page.getByText(searchHintName)).toBeVisible()
  })

  test('should display pagination when results exist', async ({ page }) => {
    const searchInput = page.getByPlaceholder(searchPlaceholderName)
    await searchInput.fill('test')
    await page.locator('form').getByRole('button', { name: searchButtonName }).click()

    await expect(page.getByRole('button', { name: prevPageName })).toBeVisible()
    await expect(page.getByRole('button', { name: nextPageName })).toBeVisible()
  })

  test('should toggle tag filter', async ({ page }) => {
    const workTag = page.locator('button.rounded-full').filter({ hasText: 'Work' }).first()
    await expect(workTag).toBeVisible()
    await workTag.click()
    await expect(page.getByText(selectedTagsName)).toBeVisible()
  })
})
