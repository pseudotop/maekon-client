import { i18nRegex } from './helpers/i18n'
import { mockDynamicJson, mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const dashboardHeadingName = i18nRegex('dashboard.title')
const timelineHeadingName = i18nRegex('timeline.title')

async function mockResponsiveApis(page: Page) {
  await mockStaticJson(page, '**/api/stats/summary**', {
    date: '2026-02-23',
    total_active_secs: 3600,
    total_idle_secs: 600,
    top_apps: [{ name: 'Code', duration_secs: 1800, event_count: 3, frame_count: 2 }],
    cpu_avg: 18.2,
    memory_avg_percent: 40.1,
    frames_captured: 2,
    events_logged: 3,
  })
  await mockStaticJson(page, '**/api/metrics/hourly**', [
    {
      hour: '10:00',
      cpu_avg: 18.2,
      cpu_max: 25.1,
      memory_avg: 40.1,
      memory_max: 44.3,
      sample_count: 4,
    },
  ])
  await mockStaticJson(page, '**/api/processes**', [
    {
      timestamp: '2026-02-23T10:00:00Z',
      processes: [{ pid: 1001, name: 'Code', cpu_usage: 10.2, memory_bytes: 345678912 }],
    },
  ])
  await mockStaticJson(page, '**/api/focus/metrics**', {
    today: {
      date: '2026-02-23',
      total_active_secs: 3600,
      deep_work_secs: 1800,
      communication_secs: 300,
      context_switches: 4,
      interruption_count: 1,
      avg_focus_duration_secs: 900,
      max_focus_duration_secs: 1800,
      focus_score: 78,
    },
    history: [],
  })
  await mockStaticJson(page, '**/api/focus/suggestions**', [])
  await mockStaticJson(page, '**/api/stats/heatmap**', {
    from_date: '2026-02-17',
    to_date: '2026-02-23',
    cells: [{ day: 1, hour: 10, value: 1 }],
    max_value: 1,
  })
  await mockStaticJson(page, '**/api/update/status**', {
    enabled: true,
    auto_install: false,
    phase: 'Idle',
    message: null,
    pending: null,
    revision: 1,
    updated_at: '2026-02-23T10:00:00Z',
  })
  await mockStaticJson(page, '**/api/tags**', [])
  await mockStaticJson(page, '**/api/frames**', {
    data: [
      {
        id: 1,
        timestamp: '2026-02-23T10:00:00Z',
        trigger_type: 'interval',
        app_name: 'Code',
        window_title: 'ONESHIM',
        importance: 0.8,
        resolution: '1920x1080',
        file_path: null,
        ocr_text: null,
        image_url: null,
        tag_ids: [],
      },
    ],
    pagination: { total: 1, offset: 0, limit: 50, has_more: false },
  })
  await mockDynamicJson(page, '**/api/search**', async (request) => {
    const url = new URL(request.url())
    const query = url.searchParams.get('q') ?? ''
    return {
      query,
      total: 1,
      offset: 0,
      limit: 20,
      results: [
        {
          result_type: 'frame',
          id: '1',
          timestamp: '2026-02-23T10:00:00Z',
          app_name: 'Code',
          window_title: 'ONESHIM',
          matched_text: query || 'focus',
          image_url: null,
          importance: 0.8,
          tags: [],
        },
      ],
    }
  })
}

test.describe('Responsive', () => {
  test('supports small viewport with ActivityBar navigation', async ({ page }) => {
    await page.setViewportSize({ width: 430, height: 932 })
    await mockResponsiveApis(page)

    await page.goto('/')
    await expect(page.getByRole('heading', { name: dashboardHeadingName })).toBeVisible()
    const shellBox = await page.locator('.app-shell').boundingBox()
    expect(shellBox?.width).toBeLessThanOrEqual((page.viewportSize()?.width ?? 430) + 1)

    // The monitor group is already active on / so the SidePanel tree is
    // rendered.  Drill into "All Frames" (unique label) to navigate into
    // /timeline/all — this exercises the ActivityBar + SidePanel pair on a
    // narrow viewport without needing to toggle the already-active group.
    const tree = page.locator('[role="tree"]')
    await expect(tree).toBeVisible()
    await tree.getByRole('treeitem', { name: /all frames/i }).click()

    await expect(page).toHaveURL(/\/timeline\/all/)
    await expect(page.getByRole('heading', { name: timelineHeadingName })).toBeVisible()
  })

  test('supports tablet viewport with CommandPalette', async ({ page }) => {
    await page.setViewportSize({ width: 768, height: 1024 })
    await mockResponsiveApis(page)

    await page.goto('/')
    await expect(page.getByRole('heading', { name: dashboardHeadingName })).toBeVisible()

    // Open command palette with keyboard shortcut
    await page.keyboard.press('Meta+k')
    const input = page.getByRole('dialog').locator('input')
    await expect(input).toBeVisible()
  })
})
