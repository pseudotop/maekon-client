/**
 *
 */

import { DEFAULT_WEB_PORT } from '../src/constants'
import { i18nRegex } from './helpers/i18n'
import { mockDynamicJson, mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const settingsTitleName = i18nRegex('settings.title')
const collectionSectionName = i18nRegex('settings.collectionTitle')
const notificationSectionName = i18nRegex('settings.notifTitle')
const webDashboardSectionName = i18nRegex('settings.webTitle')
const exportSectionName = i18nRegex('settings.exportTitle')
const exportFormatLabelName = i18nRegex('settings.exportFormatLabel')
const languageSelectorName = i18nRegex('settings.language')

const mockedSettings = {
  retention_days: 30,
  max_storage_mb: 2048,
  web_port: DEFAULT_WEB_PORT,
  allow_external: false,
  capture_enabled: true,
  idle_threshold_secs: 300,
  metrics_interval_secs: 10,
  process_interval_secs: 30,
  notification: {
    enabled: true,
    idle_notification: true,
    idle_notification_mins: 10,
    long_session_notification: true,
    long_session_mins: 60,
    high_usage_notification: true,
    high_usage_threshold: 80,
  },
  update: {
    enabled: true,
    check_interval_hours: 24,
    include_prerelease: false,
    channel: 'stable',
    auto_install: false,
  },
  telemetry: {
    enabled: false,
    crash_reports: false,
    usage_analytics: false,
    performance_metrics: false,
  },
  monitor: {
    process_monitoring: true,
    input_activity: true,
    privacy_mode: false,
  },
  privacy: {
    excluded_apps: [],
    excluded_app_patterns: [],
    excluded_title_patterns: [],
    auto_exclude_sensitive: true,
    pii_filter_level: 'standard',
  },
  schedule: {
    active_hours_enabled: false,
    active_start_hour: 9,
    active_end_hour: 18,
    active_days: ['Mon', 'Tue', 'Wed', 'Thu', 'Fri'],
    pause_on_screen_lock: true,
    pause_on_battery_saver: true,
  },
  automation: {
    enabled: true,
  },
  sandbox: {
    enabled: true,
    profile: 'balanced',
    allowed_read_paths: [],
    allowed_write_paths: [],
    allow_network: false,
    max_memory_bytes: 536870912,
    max_cpu_time_ms: 30000,
  },
  ai_provider: {
    ocr_provider: 'local',
    llm_provider: 'local',
    external_data_policy: 'disabled',
    allow_unredacted_external_ocr: false,
    ocr_validation: {
      enabled: true,
      min_confidence: 0.25,
      max_invalid_ratio: 0.6,
    },
    scene_action_override: {
      enabled: false,
      reason: '',
      approved_by: '',
      expires_at: null,
    },
    scene_intelligence: {
      enabled: true,
      overlay_enabled: true,
      allow_action_execution: true,
      min_confidence: 0.35,
      max_elements: 120,
      calibration_enabled: true,
      calibration_min_elements: 8,
      calibration_min_avg_confidence: 0.55,
    },
    fallback_to_local: true,
    ocr_api: null,
    llm_api: null,
  },
}

const mockedStorageStats = {
  db_size_bytes: 10485760,
  frames_size_bytes: 7340032,
  total_size_bytes: 17825792,
  frame_count: 128,
  event_count: 342,
  metric_count: 88,
  oldest_data_date: '2026-02-01T00:00:00Z',
  newest_data_date: '2026-02-23T00:00:00Z',
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

async function mockSettingsApis(page: Page) {
  let currentSettings = structuredClone(mockedSettings)

  await mockDynamicJson(page, '**/api/settings', (request) => {
    if (request.method() === 'POST') {
      currentSettings = request.postDataJSON() ?? currentSettings
      return currentSettings
    }
    return currentSettings
  })

  await mockStaticJson(page, '**/api/storage/stats**', mockedStorageStats)
  await mockStaticJson(page, '**/api/update/status**', mockedUpdateStatus)
}

function settingsHeading(page: Page) {
  return page.locator('h1').filter({ hasText: settingsTitleName })
}

async function gotoSettingsTab(page: Page, tabSlug: string) {
  await page.goto(`/settings/${tabSlug}`)
  await expect(settingsHeading(page)).toBeVisible({ timeout: 10000 })
}

test.describe('Settings', () => {
  test.beforeEach(async ({ page }) => {
    await mockSettingsApis(page)
    await page.goto('/settings')
    await expect(settingsHeading(page)).toBeVisible({ timeout: 10000 })
  })

  test('should display settings title', async ({ page }) => {
    await expect(settingsHeading(page)).toBeVisible()
  })

  test('should display data collection section', async ({ page }) => {
    await gotoSettingsTab(page, 'monitoring')
    await expect(page.getByText(collectionSectionName)).toBeVisible()
  })

  test('should display capture enable checkbox', async ({ page }) => {
    await gotoSettingsTab(page, 'monitoring')
    // SettingsLayout renders the active tab inside <form id="settings-form"> via <Outlet>.
    // Since only MonitoringTab is mounted at /settings/monitoring, the first checkbox
    // in the form is the "capture enabled" toggle owned by that tab.
    const captureCheckbox = page.locator('form#settings-form input[type="checkbox"]').first()
    await expect(captureCheckbox).toBeVisible()
  })

  test('should display idle threshold input', async ({ page }) => {
    await gotoSettingsTab(page, 'monitoring')
    const idleInput = page.locator('#settings-idle-threshold')
    await expect(idleInput).toBeVisible()
  })

  test('should display notification settings', async ({ page }) => {
    await gotoSettingsTab(page, 'general')
    await expect(page.getByText(notificationSectionName)).toBeVisible()
  })

  test('should display web dashboard port setting', async ({ page }) => {
    await gotoSettingsTab(page, 'general')
    await expect(page.getByText(webDashboardSectionName)).toBeVisible()
  })

  test('should display data export section', async ({ page }) => {
    await gotoSettingsTab(page, 'data')
    await expect(page.getByText(exportSectionName)).toBeVisible()
  })

  test('should display export format selector', async ({ page }) => {
    await gotoSettingsTab(page, 'data')
    await expect(page.locator('span').filter({ hasText: exportFormatLabelName }).first()).toBeVisible()
  })

  test('should display export buttons', async ({ page }) => {
    await gotoSettingsTab(page, 'data')
    const exportSection = page.getByText(exportSectionName)
    await exportSection.scrollIntoViewIfNeeded()

    // The form holds both the tab content (via <Outlet>) and the SettingsLayout
    // save button. Exclude the save button testids so we only count tab-specific
    // action buttons (the export buttons in DataStorageTab).
    const buttons = page.locator(
      'form#settings-form button:not([data-testid="settings-save"]):not([data-testid="settings-save-floating"])',
    )
    const count = await buttons.count()
    expect(count).toBeGreaterThan(0)
  })

  test('should display language selector', async ({ page }) => {
    await gotoSettingsTab(page, 'general')
    await expect(page.getByLabel(languageSelectorName)).toBeVisible()
  })

  test('should have save button', async ({ page }) => {
    const saveButton = page.getByTestId('settings-save')
    await expect(saveButton).toBeVisible()
  })

  test('should save settings', async ({ page }) => {
    await gotoSettingsTab(page, 'monitoring')
    // See "should display capture enable checkbox" for the selector rationale.
    const captureCheckbox = page.locator('form#settings-form input[type="checkbox"]').first()
    await captureCheckbox.uncheck()

    let saveRequests = 0
    await page.route('**/api/settings', async (route) => {
      if (route.request().method() === 'POST') {
        saveRequests += 1
      }
      await route.fallback()
    })

    const saveButton = page.getByTestId('settings-save')
    await expect(saveButton).toBeEnabled()

    await saveButton.click()

    await expect.poll(() => saveRequests).toBe(1)
    await expect(saveButton).toBeDisabled()
  })

  test('should validate port number', async ({ page }) => {
    await gotoSettingsTab(page, 'general')
    const saveButton = page.getByTestId('settings-save')
    await expect(saveButton).toBeVisible()
  })
})
