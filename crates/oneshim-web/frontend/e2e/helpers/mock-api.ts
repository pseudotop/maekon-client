import type { Page, Request, Route } from '@playwright/test'
import { DEFAULT_WEB_PORT } from '../../src/constants'

type RoutePattern = Parameters<Page['route']>[0]
type JsonResolver = (request: Request) => unknown | Promise<unknown>

const STREAM_HEADERS = {
  'cache-control': 'no-cache',
  connection: 'keep-alive',
  'content-type': 'text/event-stream',
}

async function fulfillJson(route: Route, payload: unknown, status = 200): Promise<void> {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(payload),
  })
}

export async function mockStaticJson(page: Page, pattern: RoutePattern, payload: unknown, status = 200): Promise<void> {
  await page.route(pattern, async (route) => {
    await fulfillJson(route, payload, status)
  })
}

export async function mockDynamicJson(
  page: Page,
  pattern: RoutePattern,
  resolver: JsonResolver,
  status = 200,
): Promise<void> {
  await page.route(pattern, async (route) => {
    const payload = await resolver(route.request())
    await fulfillJson(route, payload, status)
  })
}

export async function mockBackgroundStreams(page: Page): Promise<void> {
  await page.route('**/api/stream**', async (route) => {
    await route.fulfill({
      status: 200,
      headers: STREAM_HEADERS,
      body: 'event: message\ndata: {}\n\n',
    })
  })

  await page.route('**/api/update/stream**', async (route) => {
    await route.fulfill({
      status: 200,
      headers: STREAM_HEADERS,
      body: 'event: message\ndata: {}\n\n',
    })
  })
}

const fallbackSettings = {
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

const fallbackUpdateStatus = {
  enabled: true,
  auto_install: false,
  phase: 'Idle',
  message: null,
  pending: null,
  revision: 1,
  updated_at: '2026-02-23T10:00:00Z',
}

const fallbackStorageStats = {
  db_size_bytes: 524288,
  frames_size_bytes: 262144,
  total_size_bytes: 786432,
  frame_count: 1,
  event_count: 3,
  metric_count: 0,
  oldest_data_date: '2026-02-23T09:55:00Z',
  newest_data_date: '2026-02-23T10:05:00Z',
}

const fallbackSummary = {
  date: '2026-02-23',
  total_active_secs: 14400,
  total_idle_secs: 1800,
  top_apps: [
    { name: 'VS Code', duration_secs: 7200, event_count: 42, frame_count: 18 },
    { name: 'Chrome', duration_secs: 3600, event_count: 24, frame_count: 12 },
  ],
  cpu_avg: 21.5,
  memory_avg_percent: 48.2,
  frames_captured: 36,
  events_logged: 128,
}

const fallbackReport = {
  title: 'Fallback Report',
  from_date: '2026-02-17',
  to_date: '2026-02-23',
  days: 7,
  total_active_secs: 0,
  total_idle_secs: 0,
  total_captures: 0,
  total_events: 0,
  avg_cpu: 0,
  avg_memory: 0,
  daily_stats: [],
  app_stats: [],
  hourly_activity: [],
  productivity: {
    score: 0,
    active_ratio: 0,
    peak_hour: 0,
    top_app: '',
    trend: 0,
  },
}

const fallbackPreviewImage =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9sWwaP8AAAAASUVORK5CYII='

const fallbackFrames = {
  data: [
    {
      id: 101,
      timestamp: '2026-02-23T09:55:00Z',
      trigger_type: 'active_window',
      app_name: 'VS Code',
      window_title: 'release-flow-hardening.ts',
      importance: 0.82,
      resolution: '1280x720',
      file_path: null,
      ocr_text: 'Release flow hardening draft',
      image_url: fallbackPreviewImage,
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

const fallbackTimeline = {
  session: {
    start: '2026-02-23T09:55:00Z',
    end: '2026-02-23T10:15:00Z',
    duration_secs: 1200,
    total_events: 2,
    total_frames: 1,
    total_idle_secs: 0,
  },
  items: [
    {
      type: 'Event',
      id: 'evt-101',
      timestamp: '2026-02-23T10:00:00Z',
      event_type: 'app_switch',
      app_name: 'VS Code',
      window_title: 'release-flow-hardening.ts',
    },
    {
      type: 'Frame',
      id: 101,
      timestamp: '2026-02-23T09:55:00Z',
      app_name: 'VS Code',
      window_title: 'release-flow-hardening.ts',
      importance: 0.82,
      image_url: fallbackPreviewImage,
    },
  ],
  segments: [
    {
      app_name: 'VS Code',
      start: '2026-02-23T09:55:00Z',
      end: '2026-02-23T10:15:00Z',
      color: '#14b8a6',
    },
  ],
}

const fallbackFocusMetrics = {
  today: {
    date: '2026-02-23',
    total_active_secs: 12600,
    deep_work_secs: 5400,
    communication_secs: 1800,
    context_switches: 6,
    interruption_count: 3,
    avg_focus_duration_secs: 2100,
    max_focus_duration_secs: 4200,
    focus_score: 78,
  },
  history: [
    {
      date: '2026-02-21',
      total_active_secs: 10800,
      deep_work_secs: 4200,
      communication_secs: 2400,
      context_switches: 9,
      interruption_count: 4,
      avg_focus_duration_secs: 1800,
      max_focus_duration_secs: 3600,
      focus_score: 68,
    },
    {
      date: '2026-02-22',
      total_active_secs: 11700,
      deep_work_secs: 4800,
      communication_secs: 2100,
      context_switches: 7,
      interruption_count: 3,
      avg_focus_duration_secs: 1950,
      max_focus_duration_secs: 3900,
      focus_score: 72,
    },
  ],
}

const fallbackAutomationStatus = {
  enabled: true,
  sandbox_enabled: true,
  sandbox_profile: 'balanced',
  ocr_provider: 'local',
  llm_provider: 'local',
  ocr_source: 'local',
  llm_source: 'local',
  ocr_fallback_reason: null,
  llm_fallback_reason: null,
  external_data_policy: 'disabled',
  pending_audit_entries: 0,
}

const fallbackAuditLogs = []

const fallbackAutomationPresets = {
  presets: [
    {
      id: 'preset-productivity',
      name: 'Focus Session',
      description: 'Start a focused productivity workflow.',
      category: 'Productivity',
      steps: [{ name: 'Start focus timer', intent: { action: 'focus.start' }, delay_ms: 0, stop_on_failure: true }],
      builtin: true,
      platform: null,
    },
    {
      id: 'preset-app-management',
      name: 'Close Distracting Apps',
      description: 'Close a predefined set of distracting apps.',
      category: 'AppManagement',
      steps: [
        { name: 'Close social apps', intent: { action: 'apps.close_social' }, delay_ms: 0, stop_on_failure: true },
      ],
      builtin: true,
      platform: null,
    },
    {
      id: 'preset-workflow',
      name: 'Daily Review',
      description: 'Open the daily review checklist workflow.',
      category: 'Workflow',
      steps: [
        { name: 'Open review checklist', intent: { action: 'workflow.review' }, delay_ms: 0, stop_on_failure: true },
      ],
      builtin: true,
      platform: null,
    },
    {
      id: 'preset-custom',
      name: 'Custom Macro',
      description: 'Run a user-defined custom macro.',
      category: 'Custom',
      steps: [{ name: 'Execute custom macro', intent: { action: 'custom.run' }, delay_ms: 0, stop_on_failure: true }],
      builtin: false,
      platform: null,
    },
  ],
}

export async function mockDefaultApiFallbacks(page: Page): Promise<void> {
  await mockDynamicJson(page, '**/api/settings', (request) =>
    request.method() === 'POST' ? (request.postDataJSON() ?? fallbackSettings) : fallbackSettings,
  )
  await mockStaticJson(page, '**/api/update/status**', fallbackUpdateStatus)
  await mockStaticJson(page, '**/api/storage/stats**', fallbackStorageStats)
  await mockStaticJson(page, '**/api/stats/summary**', fallbackSummary)
  await mockStaticJson(page, '**/api/metrics/hourly**', [
    { hour: '2026-02-23T09:00:00Z', cpu_avg: 18, cpu_max: 26, memory_avg: 42, memory_max: 48, sample_count: 6 },
    { hour: '2026-02-23T10:00:00Z', cpu_avg: 22, cpu_max: 31, memory_avg: 47, memory_max: 53, sample_count: 6 },
    { hour: '2026-02-23T11:00:00Z', cpu_avg: 19, cpu_max: 28, memory_avg: 45, memory_max: 50, sample_count: 6 },
  ])
  await mockStaticJson(page, '**/api/stats/heatmap**', {
    from_date: '2026-02-17',
    to_date: '2026-02-23',
    max_value: 9,
    cells: [
      { day: 0, hour: 9, value: 4 },
      { day: 1, hour: 10, value: 6 },
      { day: 2, hour: 11, value: 8 },
      { day: 3, hour: 13, value: 9 },
      { day: 4, hour: 15, value: 7 },
      { day: 5, hour: 16, value: 5 },
    ],
  })
  await mockStaticJson(page, '**/api/processes**', [
    {
      timestamp: '2026-02-23T10:00:00Z',
      processes: [
        { pid: 101, name: 'Code Helper', cpu_usage: 12.4, memory_bytes: 734003200 },
        { pid: 102, name: 'Google Chrome', cpu_usage: 8.1, memory_bytes: 524288000 },
      ],
    },
  ])
  await mockStaticJson(page, '**/api/tags**', [])
  await mockStaticJson(page, '**/api/frames**', fallbackFrames)
  await mockStaticJson(page, '**/api/timeline**', fallbackTimeline)
  await mockStaticJson(page, '**/api/frames/*/tags**', [])
  await mockStaticJson(page, '**/api/frames/*/tags/*', {})
  await mockStaticJson(page, '**/api/reports**', fallbackReport)
  await mockStaticJson(page, '**/api/focus/metrics**', fallbackFocusMetrics)
  await mockStaticJson(page, '**/api/focus/sessions**', [
    {
      id: 1,
      started_at: '2026-02-23T09:00:00Z',
      ended_at: '2026-02-23T10:15:00Z',
      primary_app: 'VS Code',
      category: 'Development',
      state: 'completed',
      interruption_count: 1,
      deep_work_secs: 3600,
      duration_secs: 4500,
    },
  ])
  await mockStaticJson(page, '**/api/focus/interruptions**', [
    {
      id: 1,
      interrupted_at: '2026-02-23T10:20:00Z',
      from_app: 'VS Code',
      from_category: 'Development',
      to_app: 'Slack',
      to_category: 'Communication',
      resumed_at: '2026-02-23T10:24:00Z',
      resumed_to_app: 'VS Code',
      duration_secs: 240,
    },
  ])
  await mockStaticJson(page, '**/api/focus/suggestions**', [])
  await mockStaticJson(page, '**/api/ai/provider-surfaces**', {
    version: 1,
    updated_at: '2026-03-01T00:00:00Z',
    vendors: [],
    surfaces: [],
  })
  await mockStaticJson(page, '**/api/automation/status**', fallbackAutomationStatus)
  await mockStaticJson(page, '**/api/automation/audit**', fallbackAuditLogs)
  await mockStaticJson(page, '**/api/automation/stats**', {
    total_executions: 0,
    successful: 0,
    failed: 0,
    denied: 0,
    timeout: 0,
    avg_elapsed_ms: 0,
    success_rate: 0,
    blocked_rate: 0,
    p95_elapsed_ms: 0,
    timing_samples: 0,
  })
  await mockStaticJson(page, '**/api/automation/policies**', {
    automation_enabled: true,
    sandbox_profile: 'Standard',
    sandbox_enabled: true,
    allow_network: false,
    external_data_policy: 'PiiFilterStandard',
    scene_action_override_enabled: false,
    scene_action_override_active: false,
    scene_action_override_reason: null,
    scene_action_override_approved_by: null,
    scene_action_override_expires_at: null,
    scene_action_override_issue: null,
  })
  await mockStaticJson(page, '**/api/automation/contracts**', {
    audit_schema_version: 'automation.audit.v1',
    scene_schema_version: 'ui_scene.v1',
    scene_action_schema_version: 'automation.scene_action.v1',
  })
  await mockStaticJson(page, '**/api/automation/presets', fallbackAutomationPresets)
  await mockStaticJson(page, '**/api/automation/scene**', {
    schema_version: 'ui_scene.v1',
    scene_id: 'scene-e2e',
    app_name: 'oneshim-e2e',
    screen_id: 'screen-main',
    captured_at: '2026-02-23T10:00:00Z',
    screen_width: 1280,
    screen_height: 720,
    elements: [],
  })
  await mockStaticJson(page, '**/api/automation/scene/calibration**', {
    schema_version: 'automation.scene_calibration.v1',
    scene_id: 'scene-e2e',
    total_elements: 0,
    considered_elements: 0,
    avg_confidence: 0,
    min_confidence: 0.35,
    min_required_elements: 8,
    min_required_avg_confidence: 0.55,
    passed: false,
    reasons: ['insufficient elements: 0 < 8'],
  })
  await mockDynamicJson(page, '**/api/automation/execute-hint', (request) => {
    const payload = request.postDataJSON() as
      | { command_id?: string; session_id?: string; intent_hint?: string }
      | undefined
    return {
      command_id: payload?.command_id ?? 'intent-hint-e2e',
      session_id: payload?.session_id ?? 'sess-e2e',
      planned_intent: {
        ClickElement: {
          text: payload?.intent_hint ?? null,
          role: null,
          app_name: null,
          button: 'left',
        },
      },
      result: {
        success: true,
        element: null,
        verification: null,
        retry_count: 0,
        elapsed_ms: 0,
        error: null,
      },
    }
  })
  await mockDynamicJson(page, '**/api/automation/execute-scene-action', (request) => {
    const payload = request.postDataJSON() as
      | {
          command_id?: string
          session_id?: string
          frame_id?: number
          scene_id?: string
          element_id?: string
          action_type?: 'click' | 'type_text'
          text?: string
        }
      | undefined

    return {
      schema_version: 'automation.scene_action.v1',
      command_id: payload?.command_id ?? 'scene-action-e2e',
      session_id: payload?.session_id ?? 'sess-e2e',
      frame_id: payload?.frame_id ?? 0,
      scene_id: payload?.scene_id ?? 'scene-e2e',
      element_id: payload?.element_id ?? 'el-e2e',
      applied_privacy_policy: 'AllowFiltered',
      scene_action_override_active: false,
      scene_action_override_expires_at: null,
      executed_intents:
        payload?.action_type === 'type_text'
          ? [
              { Raw: { MouseClick: { button: 'left', x: 100, y: 100 } } },
              { Raw: { KeyType: { text: payload?.text ?? '' } } },
            ]
          : [{ Raw: { MouseClick: { button: 'left', x: 100, y: 100 } } }],
      result: {
        success: true,
        element: null,
        verification: null,
        retry_count: 0,
        elapsed_ms: 0,
        error: null,
      },
    }
  })
}
