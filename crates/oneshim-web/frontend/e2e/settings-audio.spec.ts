/**
 * E2E tests for the AudioTab in Settings (/settings/audio).
 *
 * Covers tab navigation, enable toggle, model size selector,
 * language selector, download button, STT provider radio group,
 * input mode selection, and cloud fallback section.
 */

import { DEFAULT_WEB_PORT } from '../src/constants'
import { i18nRegex } from './helpers/i18n'
import { mockDynamicJson, mockStaticJson } from './helpers/mock-api'
import { expect, type Page, test } from './helpers/test'

const settingsTitleName = i18nRegex('settings.title')

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
  audio: {
    enabled: true,
    whisper_model_path: '',
    language: 'auto',
    max_recording_secs: 120,
    model_size: 'base',
    stt_provider: 'local',
    cloud_api_key: '',
    cloud_stt_endpoint: '',
    cloud_timeout_secs: 30,
    mic_input_mode: 'push_to_talk',
    vad_threshold: 0.02,
    vad_silence_ms: 800,
  },
}

async function mockSettingsApis(page: Page) {
  await mockDynamicJson(page, '**/api/settings', (request) => {
    if (request.method() === 'POST') {
      return request.postDataJSON() ?? mockedSettings
    }
    return mockedSettings
  })
  await mockStaticJson(page, '**/api/storage/stats**', {
    db_size_bytes: 524288,
    frames_size_bytes: 262144,
    total_size_bytes: 786432,
    frame_count: 1,
    event_count: 3,
    metric_count: 0,
    oldest_data_date: '2026-02-23T09:55:00Z',
    newest_data_date: '2026-02-23T10:05:00Z',
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
}

function settingsHeading(page: Page) {
  return page.locator('h1').filter({ hasText: settingsTitleName })
}

async function gotoAudioTab(page: Page) {
  await page.goto('/settings/audio')
  await expect(settingsHeading(page)).toBeVisible({ timeout: 10000 })
}

/**
 * Returns the AudioTab scope. SettingsLayout renders the active tab inside
 * <form id="settings-form"> via <Outlet>, so at /settings/audio the form is
 * the natural panel scope (only AudioTab is mounted).
 */
function audioPanel(page: Page) {
  return page.locator('form#settings-form')
}

test.describe('Settings Audio Tab', () => {
  test.beforeEach(async ({ page }) => {
    await mockSettingsApis(page)
  })

  test('should display audio tab panel', async ({ page }) => {
    await gotoAudioTab(page)

    // The legacy `#settings-panel-audio` wrapper was removed when SettingsLayout
    // switched to <Outlet>. Verify AudioTab actually rendered by asserting an
    // audio-specific control exists.
    await expect(page.locator('#audio-model-size')).toBeVisible()
  })

  test('should display audio section heading', async ({ page }) => {
    await gotoAudioTab(page)

    // AudioTab renders an h3 with fallback text "Audio & Speech-to-Text"
    const heading = audioPanel(page).locator('h3').first()
    await expect(heading).toBeVisible()
    await expect(heading).toContainText(/Audio/i)
  })

  test('should display enable audio checkbox', async ({ page }) => {
    await gotoAudioTab(page)

    const panel = audioPanel(page)
    const checkbox = panel.locator('input[type="checkbox"]').first()
    await expect(checkbox).toBeVisible()
    // Should be checked since mockedSettings.audio.enabled = true
    await expect(checkbox).toBeChecked()
  })

  test('should display model size selector with options', async ({ page }) => {
    await gotoAudioTab(page)

    const modelSelect = page.locator('#audio-model-size')
    await expect(modelSelect).toBeVisible()

    // Verify all four model size options
    await expect(modelSelect.locator('option[value="tiny"]')).toHaveCount(1)
    await expect(modelSelect.locator('option[value="base"]')).toHaveCount(1)
    await expect(modelSelect.locator('option[value="small"]')).toHaveCount(1)
    await expect(modelSelect.locator('option[value="medium"]')).toHaveCount(1)

    // Default should be 'base' from mockedSettings
    await expect(modelSelect).toHaveValue('base')
  })

  test('should display language selector', async ({ page }) => {
    await gotoAudioTab(page)

    const langSelect = page.locator('#audio-language')
    await expect(langSelect).toBeVisible()

    // Verify language options
    await expect(langSelect.locator('option[value="auto"]')).toHaveCount(1)
    await expect(langSelect.locator('option[value="en"]')).toHaveCount(1)
    await expect(langSelect.locator('option[value="ko"]')).toHaveCount(1)

    await expect(langSelect).toHaveValue('auto')
  })

  test('should display download button', async ({ page }) => {
    await gotoAudioTab(page)

    // The AudioTab always renders a download or re-download button
    const panel = audioPanel(page)
    const downloadBtn = panel.locator('button').filter({ hasText: /Download|Re-download/i })
    await expect(downloadBtn).toBeVisible()
  })

  test('should display STT provider radio group', async ({ page }) => {
    await gotoAudioTab(page)

    const panel = audioPanel(page)

    // Local (Whisper) radio
    const localRadio = panel.locator('input[name="stt_provider"][value="local"]')
    await expect(localRadio).toBeVisible()
    await expect(localRadio).toBeChecked()

    // Cloud (OpenAI) radio
    const cloudRadio = panel.locator('input[name="stt_provider"][value="cloud"]')
    await expect(cloudRadio).toBeVisible()
    await expect(cloudRadio).not.toBeChecked()
  })

  test('should show cloud API key field when cloud provider selected', async ({ page }) => {
    await gotoAudioTab(page)

    const panel = audioPanel(page)

    // API key field should be hidden when local is selected
    await expect(page.locator('#cloud-api-key')).toBeHidden()

    // Select cloud provider
    const cloudRadio = panel.locator('input[name="stt_provider"][value="cloud"]')
    await cloudRadio.click()

    // Now the API key input should appear
    const apiKeyInput = page.locator('#cloud-api-key')
    await expect(apiKeyInput).toBeVisible()
    await expect(apiKeyInput).toHaveAttribute('type', 'password')
    await expect(apiKeyInput).toHaveAttribute('placeholder', 'sk-...')
  })

  test('should display input mode radio group with PTT and VAD options', async ({ page }) => {
    await gotoAudioTab(page)

    const panel = audioPanel(page)

    // Push-to-Talk radio
    const pttRadio = panel.locator('input[name="mic_input_mode"][value="push_to_talk"]')
    await expect(pttRadio).toBeVisible()
    await expect(pttRadio).toBeChecked()

    // Voice Activity radio
    const vadRadio = panel.locator('input[name="mic_input_mode"][value="voice_activity"]')
    await expect(vadRadio).toBeVisible()
    await expect(vadRadio).not.toBeChecked()
  })

  test('should show VAD settings when voice activity mode selected', async ({ page }) => {
    await gotoAudioTab(page)

    const panel = audioPanel(page)

    // VAD threshold slider should be hidden when PTT is selected
    await expect(page.locator('#audio-vad-threshold')).toBeHidden()

    // Select voice activity mode
    const vadRadio = panel.locator('input[name="mic_input_mode"][value="voice_activity"]')
    await vadRadio.click()

    // VAD sensitivity slider and silence duration input should appear
    const vadThreshold = page.locator('#audio-vad-threshold')
    await expect(vadThreshold).toBeVisible()
    await expect(vadThreshold).toHaveAttribute('type', 'range')

    const vadSilence = page.locator('#audio-vad-silence')
    await expect(vadSilence).toBeVisible()
    await expect(vadSilence).toHaveAttribute('type', 'number')
  })
})
