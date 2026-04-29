import { i18nRegex } from './helpers/i18n'
import { mockDynamicJson, mockStaticJson } from './helpers/mock-api'
import { expect, test } from './helpers/test'

const replayTitle = i18nRegex('replay.title')
const hideOverlayLabel = i18nRegex('replay.hideOverlay')
const showOverlayLabel = i18nRegex('replay.showOverlay')
const runSuggestedActionLabel = i18nRegex('replay.runSuggestedAction')

function makeReplayTimeline() {
  const now = new Date()
  const start = new Date(now.getTime() - 60_000)
  const frameTimestamp = start.toISOString()
  const svg = encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720"><rect width="100%" height="100%" fill="#dbeafe"/><text x="120" y="140" font-size="42">Replay Frame</text></svg>',
  )

  return {
    session: {
      start: start.toISOString(),
      end: now.toISOString(),
      duration_secs: 60,
      total_events: 1,
      total_frames: 1,
      total_idle_secs: 0,
    },
    items: [
      {
        type: 'Frame',
        id: 101,
        timestamp: frameTimestamp,
        app_name: 'Oneshim',
        window_title: 'Replay Window',
        importance: 0.91,
        image_url: `data:image/svg+xml;utf8,${svg}`,
      },
    ],
    segments: [
      {
        app_name: 'Oneshim',
        start: start.toISOString(),
        end: now.toISOString(),
        color: '#14b8a6',
      },
    ],
  }
}

// After PR #378 follow-up: ReplayLayout now owns SceneAssistantPanel as a
// persistent right sidebar alongside the TimelineScrubberSection and the
// session statistics footer. That means the click-then-act flow works on
// `/replay/timeline` (where SceneViewport lives) without having to leave
// the sub-route. These tests exercise that full flow.
test.describe('Replay Scene Overlay', () => {
  test('toggles overlay, selects element, and runs structured scene action', async ({ page }) => {
    await mockStaticJson(page, '**/api/timeline**', makeReplayTimeline())
    await mockStaticJson(page, '**/api/frames/101/tags**', [])
    await mockStaticJson(page, '**/api/automation/scene**', {
      schema_version: 'ui_scene.v1',
      scene_id: 'scene-replay-e2e',
      app_name: 'Oneshim',
      screen_id: 'main',
      captured_at: '2026-02-23T10:00:00Z',
      screen_width: 1280,
      screen_height: 720,
      elements: [
        {
          element_id: 'el-save',
          bbox_abs: { x: 110, y: 90, width: 220, height: 56 },
          bbox_norm: { x: 0.085, y: 0.125, width: 0.172, height: 0.078 },
          label: 'Save',
          role: 'button',
          intent: 'execute',
          state: 'enabled',
          confidence: 0.95,
          text_masked: 'Save',
          parent_id: null,
        },
      ],
    })

    await page.goto('/replay')
    await expect(page.getByRole('heading', { name: replayTitle })).toBeVisible()

    const hideOverlay = page.getByRole('button', { name: hideOverlayLabel })
    await expect(hideOverlay).toBeVisible()
    await hideOverlay.click()
    await expect(page.getByRole('button', { name: showOverlayLabel })).toBeVisible()
    await page.getByRole('button', { name: showOverlayLabel }).click()

    await page.locator('button[title="button"]').first().click()
    await expect(page.locator('div.text-sm.font-semibold', { hasText: 'Save' }).first()).toBeVisible()

    await page.getByRole('button', { name: runSuggestedActionLabel }).click()
    await expect(page.getByText(/policy:|policy:/i)).toBeVisible()
  })

  test('shows failure feedback when scene action endpoint rejects', async ({ page }) => {
    await mockStaticJson(page, '**/api/timeline**', makeReplayTimeline())
    await mockStaticJson(page, '**/api/frames/101/tags**', [])
    await mockStaticJson(page, '**/api/automation/scene**', {
      schema_version: 'ui_scene.v1',
      scene_id: 'scene-replay-e2e',
      app_name: 'Oneshim',
      screen_id: 'main',
      captured_at: '2026-02-23T10:00:00Z',
      screen_width: 1280,
      screen_height: 720,
      elements: [
        {
          element_id: 'el-save',
          bbox_abs: { x: 110, y: 90, width: 220, height: 56 },
          bbox_norm: { x: 0.085, y: 0.125, width: 0.172, height: 0.078 },
          label: 'Save',
          role: 'button',
          intent: 'execute',
          state: 'enabled',
          confidence: 0.95,
          text_masked: 'Save',
          parent_id: null,
        },
      ],
    })
    await mockDynamicJson(
      page,
      '**/api/automation/execute-scene-action',
      () => ({ error: 'policy blocked for test' }),
      400,
    )

    await page.goto('/replay')
    await page.locator('button[title="button"]').first().click()
    await page.getByRole('button', { name: runSuggestedActionLabel }).click()
    await expect(page.getByText(/policy blocked for test/i)).toBeVisible()
  })
})
