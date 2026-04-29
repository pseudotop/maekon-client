import { DEFAULT_WEB_PORT } from '../constants'
import { palette } from '../styles/tokens'
import type {
  AppSettings,
  AutomationSettings,
  AutomationStats,
  AutomationStatus,
  BackupArchive,
  DailySummary,
  DeleteResult,
  ExecuteSceneActionRequest,
  ExecutionPolicyConfig,
  FocusMetricsResponse,
  LocalSuggestion,
  PoliciesInfo,
  ReportResponse,
  RestoreResult,
  SearchResponse,
  StorageStats,
  Tag,
  TimelineResponse,
  TrackingScheduleConfig,
  TrackingScheduleStatus,
  UiScene,
  UpdateStatus,
  WorkflowPreset,
} from './client'
import { DEFAULT_PROVIDER_SURFACE_CATALOG } from './defaultProviderSurfaceCatalog'

const API_BASE = '/api'
const STANDALONE_STORAGE_KEY = 'oneshim-web-standalone-mode'
const STANDALONE_QUERY_KEY = 'standalone'
function hasWindow(): boolean {
  return typeof window !== 'undefined'
}

function isTauriWindow(): boolean {
  return hasWindow() && '__TAURI_INTERNALS__' in window
}

function detectInitialStandaloneMode(): boolean {
  if (!hasWindow()) {
    return true
  }

  // Desktop builds should always use the live Axum/Tauri integration surface.
  // Persisted dev-only mock flags must not bleed into packaged releases.
  if (isTauriWindow()) {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '0')
    return false
  }

  const params = new URLSearchParams(window.location.search)
  const queryValue = params.get(STANDALONE_QUERY_KEY)
  if (queryValue === '0' || queryValue === 'false') {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '0')
    return false
  }
  if (queryValue === '1' || queryValue === 'true') {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, '1')
    return true
  }

  const saved = window.localStorage.getItem(STANDALONE_STORAGE_KEY)
  if (saved === '0') return false
  if (saved === '1') return true

  // Default to connected mode — standalone requires explicit opt-in via ?standalone=1
  return false
}

let standaloneMode = detectInitialStandaloneMode()

function setStandaloneMode(enabled: boolean): void {
  standaloneMode = enabled
  if (hasWindow()) {
    window.localStorage.setItem(STANDALONE_STORAGE_KEY, enabled ? '1' : '0')
  }
}

export function isStandaloneModeEnabled(): boolean {
  return standaloneMode
}

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: {
      'Content-Type': 'application/json',
    },
  })
}

function textBlobResponse(payload: string, contentType: string): Response {
  return new Response(payload, {
    status: 200,
    headers: {
      'Content-Type': contentType,
    },
  })
}

function parseBodyJson(body?: BodyInit | null): unknown {
  if (!body || typeof body !== 'string') {
    return null
  }
  try {
    return JSON.parse(body)
  } catch {
    return null
  }
}

function todayIsoDate(): string {
  return new Date().toISOString().split('T')[0]
}

function makeDefaultSettings(): AppSettings {
  const automation: AutomationSettings = { enabled: true }
  return {
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
    automation,
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
      access_mode: 'provider_api_key',
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
        allow_action_execution: false,
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
    ai_session: {
      max_concurrent_sessions: 3,
      idle_timeout_secs: 300,
      session_timeout_secs: 600,
      max_retries: 3,
      max_history_turns: 100,
      health_check_interval_secs: 30,
    },
    suggestion: { enabled: true },
    indicator: { show_border: true, show_panel: true, border_opacity: 0.6 },
    analysis: {
      enabled: true,
      interval_secs: 60,
      min_confidence: 0.5,
      max_suggestions: 5,
      embedding_enabled: true,
      gui_intelligence_enabled: true,
      text_intelligence_enabled: true,
      auto_tuner_enabled: false,
    },
    network: {
      server_base_url: 'http://localhost:8000',
      request_timeout_ms: 30000,
      grpc_enabled: false,
      grpc_endpoint: 'http://localhost:50051',
      tls_enabled: false,
    },
    coaching: {
      enabled: true,
      tone: 'Gentle',
      quiet_hours: [],
      profiles: {
        FocusGuard: { enabled: true, min_interval_secs: 300 },
        TimeAware: { enabled: true, min_interval_secs: 300 },
        DeepWorkCoach: { enabled: true, min_interval_secs: 300 },
        ContextRestore: { enabled: true, min_interval_secs: 300 },
        GoalTracker: { enabled: true, min_interval_secs: 300 },
      },
      regime_goals: {},
      locale: 'en',
      overlay_mode: 'Minimal',
    },
    integration: { enabled: false, auth_profile_kind: 'None', request_timeout_secs: 30, sync_interval_secs: 60 },
    sync: {
      enabled: false,
      transport: 'None',
      interval_secs: 300,
      device_name: '',
      lan_advertise: false,
      compression_enabled: true,
    },
    audio: {
      enabled: false,
      whisper_model_path: '',
      language: 'auto',
      max_recording_secs: 60,
      model_size: 'base',
      stt_provider: 'local',
      cloud_api_key: '',
      cloud_stt_endpoint: 'https://api.openai.com/v1/audio/transcriptions',
      cloud_timeout_secs: 10,
      mic_input_mode: 'push_to_talk',
      vad_threshold: 0.02,
      vad_silence_ms: 800,
      vad_min_speech_ms: 300,
    },
    focus_auto: {
      enabled: false,
      duration_minutes: 25,
      trigger_apps: [],
      trigger_schedules: [],
      cooldown_secs: 300,
    },
  }
}

function makeDefaultUpdateStatus(): UpdateStatus {
  return {
    enabled: true,
    auto_install: false,
    phase: 'Idle',
    message: 'Standalone mode',
    pending: null,
    download_progress: null,
    revision: 1,
    updated_at: new Date().toISOString(),
  }
}

function makeDefaultSummary(): DailySummary {
  return {
    date: todayIsoDate(),
    total_active_secs: 0,
    total_idle_secs: 0,
    top_apps: [],
    cpu_avg: 0,
    memory_avg_percent: 0,
    frames_captured: 0,
    events_logged: 0,
  }
}

function makeDefaultStorageStats(): StorageStats {
  return {
    db_size_bytes: 0,
    frames_size_bytes: 0,
    total_size_bytes: 0,
    frame_count: 0,
    event_count: 0,
    metric_count: 0,
    oldest_data_date: null,
    newest_data_date: null,
  }
}

function makeDefaultFocusMetrics(): FocusMetricsResponse {
  return {
    today: {
      date: todayIsoDate(),
      total_active_secs: 0,
      deep_work_secs: 0,
      communication_secs: 0,
      context_switches: 0,
      interruption_count: 0,
      avg_focus_duration_secs: 0,
      max_focus_duration_secs: 0,
      focus_score: 0,
    },
    history: [],
  }
}

function makeDefaultTrackingSchedule(): TrackingScheduleConfig {
  return {
    enabled: false,
    windows: [],
    timezone: 'Local',
  }
}

function makeDefaultTrackingScheduleStatus(): TrackingScheduleStatus {
  return {
    active_now: false,
    ends_at: null,
    next_starts_at: null,
    label: '',
  }
}

function makeDefaultTimeline(): TimelineResponse {
  const end = new Date()
  const start = new Date(end.getTime() - 60 * 60 * 1000)
  return {
    session: {
      start: start.toISOString(),
      end: end.toISOString(),
      duration_secs: Math.floor((end.getTime() - start.getTime()) / 1000),
      total_events: 0,
      total_frames: 0,
      total_idle_secs: 0,
    },
    items: [],
    segments: [],
  }
}

function makeDefaultReport(): ReportResponse {
  return {
    title: 'Standalone Report',
    from_date: todayIsoDate(),
    to_date: todayIsoDate(),
    days: 1,
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
}

function makeDefaultAutomationStatus(): AutomationStatus {
  return {
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
}

function makeDefaultPolicies(): PoliciesInfo {
  return {
    automation_enabled: true,
    sandbox_profile: 'balanced',
    sandbox_enabled: true,
    allow_network: false,
    external_data_policy: 'disabled',
    scene_action_override_enabled: false,
    scene_action_override_active: false,
    scene_action_override_reason: null,
    scene_action_override_approved_by: null,
    scene_action_override_expires_at: null,
    scene_action_override_issue: null,
  }
}

function makeDefaultAutomationStats(): AutomationStats {
  return {
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
  }
}

function makeDefaultAutomationScene(appName?: string, screenId?: string, frameId?: number): UiScene {
  return {
    schema_version: 'ui_scene.v1',
    scene_id: `scene-standalone-${frameId ?? Date.now()}`,
    app_name: appName ?? null,
    screen_id: screenId ?? null,
    captured_at: new Date().toISOString(),
    screen_width: 1920,
    screen_height: 1080,
    elements: [
      {
        element_id: 'el-standalone-save',
        bbox_abs: { x: 128, y: 96, width: 220, height: 48 },
        bbox_norm: { x: 0.0667, y: 0.0889, width: 0.1146, height: 0.0444 },
        label: 'Save',
        role: 'button',
        intent: 'execute',
        state: 'enabled',
        confidence: 0.92,
        text_masked: 'Save',
        parent_id: null,
      },
    ],
  }
}

type StandaloneState = {
  settings: AppSettings
  updateStatus: UpdateStatus
  tags: Tag[]
  frameTags: Map<number, Set<number>>
  suggestions: LocalSuggestion[]
  presets: WorkflowPreset[]
  executionPolicies: ExecutionPolicyConfig[]
  trackingSchedule: TrackingScheduleConfig
  nextTagId: number
}

const state: StandaloneState = {
  settings: makeDefaultSettings(),
  updateStatus: makeDefaultUpdateStatus(),
  tags: [],
  frameTags: new Map(),
  suggestions: [],
  presets: [],
  executionPolicies: [],
  trackingSchedule: makeDefaultTrackingSchedule(),
  nextTagId: 1,
}

function getFrameTagList(frameId: number): Tag[] {
  const tagIds = state.frameTags.get(frameId)
  if (!tagIds) return []
  return state.tags.filter((tag) => tagIds.has(tag.id))
}

function ensureFrameTagSet(frameId: number): Set<number> {
  const existing = state.frameTags.get(frameId)
  if (existing) return existing
  const created = new Set<number>()
  state.frameTags.set(frameId, created)
  return created
}

function parseId(value: string | undefined): number | null {
  if (!value) return null
  const parsed = Number(value)
  if (!Number.isFinite(parsed)) return null
  return parsed
}

function makeSearchResponse(url: URL): SearchResponse {
  const query = url.searchParams.get('q') ?? ''
  const limit = Number(url.searchParams.get('limit') ?? '50')
  const offset = Number(url.searchParams.get('offset') ?? '0')
  return {
    query,
    total: 0,
    offset: Number.isFinite(offset) ? offset : 0,
    limit: Number.isFinite(limit) ? limit : 50,
    results: [],
  }
}

function makeDeleteResult(): DeleteResult {
  return {
    success: true,
    events_deleted: 0,
    frames_deleted: 0,
    metrics_deleted: 0,
    process_snapshots_deleted: 0,
    idle_periods_deleted: 0,
    message: 'Standalone mode: no persisted backend data to delete',
  }
}

function makeRestoreResult(): RestoreResult {
  return {
    success: true,
    restored: {
      settings: false,
      tags: 0,
      frame_tags: 0,
      events: 0,
      frames: 0,
    },
    errors: [],
  }
}

function makeBackupArchive(): BackupArchive {
  return {
    metadata: {
      version: '1.0',
      created_at: new Date().toISOString(),
      app_version: 'standalone',
      includes: {
        settings: true,
        tags: true,
        events: true,
        frames: true,
      },
    },
    settings: {
      capture_enabled: state.settings.capture_enabled,
      capture_interval_secs: 60,
      idle_threshold_secs: state.settings.idle_threshold_secs,
      metrics_interval_secs: state.settings.metrics_interval_secs,
      web_port: state.settings.web_port,
      notification_enabled: state.settings.notification.enabled,
      idle_notification_mins: state.settings.notification.idle_notification_mins,
      long_session_notification_mins: state.settings.notification.long_session_mins,
      high_usage_threshold_percent: state.settings.notification.high_usage_threshold,
    },
    tags: state.tags.map((tag) => ({ ...tag })),
    frame_tags: [],
    events: [],
    frames: [],
  }
}

export async function handleStandaloneRequest(
  url: string,
  options?: RequestInit,
  force = false,
): Promise<Response | null> {
  if (!standaloneMode && !force) {
    return null
  }
  if (force && !standaloneMode) {
    setStandaloneMode(true)
  }

  const requestUrl = hasWindow() ? new URL(url, window.location.origin) : new URL(url, 'http://localhost')
  const method = (options?.method ?? 'GET').toUpperCase()
  const path = requestUrl.pathname

  if (!path.startsWith(API_BASE)) {
    return null
  }

  const body = parseBodyJson(options?.body)

  if (path === '/api/stats/summary' && method === 'GET') {
    return jsonResponse(makeDefaultSummary())
  }
  if (path === '/api/metrics' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/metrics/hourly' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/processes' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/frames' && method === 'GET') {
    const limit = Number(requestUrl.searchParams.get('limit') ?? '50')
    const offset = Number(requestUrl.searchParams.get('offset') ?? '0')
    return jsonResponse({
      data: [],
      pagination: {
        total: 0,
        offset: Number.isFinite(offset) ? offset : 0,
        limit: Number.isFinite(limit) ? limit : 50,
        has_more: false,
      },
    })
  }
  if (path === '/api/events' && method === 'GET') {
    const limit = Number(requestUrl.searchParams.get('limit') ?? '100')
    const offset = Number(requestUrl.searchParams.get('offset') ?? '0')
    return jsonResponse({
      data: [],
      pagination: {
        total: 0,
        offset: Number.isFinite(offset) ? offset : 0,
        limit: Number.isFinite(limit) ? limit : 100,
        has_more: false,
      },
    })
  }
  if (path === '/api/idle' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/sessions' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/stats/apps' && method === 'GET') {
    return jsonResponse({ date: todayIsoDate(), apps: [] })
  }
  if (path === '/api/storage/stats' && method === 'GET') {
    return jsonResponse(makeDefaultStorageStats())
  }
  if (path === '/api/settings' && method === 'GET') {
    return jsonResponse(state.settings)
  }
  if (path === '/api/settings' && method === 'POST') {
    if (body && typeof body === 'object') {
      state.settings = body as AppSettings
    }
    return jsonResponse(state.settings)
  }
  if (path === '/api/ai/provider-surfaces' && method === 'GET') {
    return jsonResponse(DEFAULT_PROVIDER_SURFACE_CATALOG)
  }
  if (path === '/api/ai/providers/models' && method === 'POST') {
    const payload = body as { provider_type?: string } | null
    const provider = String(payload?.provider_type ?? 'generic')
      .trim()
      .toLowerCase()
    if (provider === 'anthropic') {
      return jsonResponse({
        models: ['claude-sonnet-4-20250514', 'claude-opus-4-1-20250805', 'claude-opus-4-20250514'],
      })
    }
    if (provider === 'google' || provider === 'gemini') {
      return jsonResponse({
        models: [
          'gemini-2.5-flash',
          'gemini-2.5-pro',
          'gemini-2.5-flash-lite',
          'gemini-3-flash-preview',
          'gemini-3-pro-preview',
        ],
      })
    }
    return jsonResponse({
      models: ['gpt-5.4', 'gpt-5.2', 'gpt-5-mini'],
    })
  }
  if (path === '/api/update/status' && method === 'GET') {
    return jsonResponse(state.updateStatus)
  }
  if (path === '/api/update/action' && method === 'POST') {
    const action = (body as { action?: string } | null)?.action
    if (action === 'CheckNow') {
      state.updateStatus = {
        ...state.updateStatus,
        phase: 'Idle',
        message: 'Standalone mode: update check skipped',
        updated_at: new Date().toISOString(),
      }
    }
    if (action === 'Approve') {
      state.updateStatus = {
        ...state.updateStatus,
        phase: 'Updated',
        message: 'Standalone mode: no binary update applied',
        updated_at: new Date().toISOString(),
      }
    }
    if (action === 'Defer') {
      state.updateStatus = {
        ...state.updateStatus,
        phase: 'Deferred',
        message: 'Standalone mode: update deferred',
        updated_at: new Date().toISOString(),
      }
    }
    return jsonResponse({ accepted: true, status: state.updateStatus })
  }
  if (path === '/api/data/range' && method === 'DELETE') {
    return jsonResponse(makeDeleteResult())
  }
  if (path === '/api/data/all' && method === 'DELETE') {
    return jsonResponse(makeDeleteResult())
  }
  if (path === '/api/search' && method === 'GET') {
    return jsonResponse(makeSearchResponse(requestUrl))
  }
  if (path === '/api/stats/heatmap' && method === 'GET') {
    const days = Number(requestUrl.searchParams.get('days') ?? '7')
    const end = new Date()
    const start = new Date(end.getTime() - Math.max(days, 1) * 24 * 60 * 60 * 1000)
    return jsonResponse({
      from_date: start.toISOString().split('T')[0],
      to_date: end.toISOString().split('T')[0],
      cells: [],
      max_value: 0,
    })
  }
  if (path.startsWith('/api/export/') && method === 'GET') {
    return textBlobResponse(
      JSON.stringify({ mode: 'standalone', exported_at: new Date().toISOString() }, null, 2),
      'application/json',
    )
  }
  if (path === '/api/tags' && method === 'GET') {
    return jsonResponse(state.tags)
  }
  if (path === '/api/tags' && method === 'POST') {
    const payload = body as { name?: string; color?: string } | null
    const newTag: Tag = {
      id: state.nextTagId++,
      name: payload?.name?.trim() || `Tag ${state.nextTagId}`,
      color: payload?.color || palette.emerald500,
      created_at: new Date().toISOString(),
    }
    state.tags = [newTag, ...state.tags]
    return jsonResponse(newTag, 201)
  }

  const tagPathMatch = path.match(/^\/api\/tags\/(\d+)$/)
  if (tagPathMatch) {
    const tagId = parseId(tagPathMatch[1])
    if (tagId == null) return jsonResponse({ error: 'Invalid tag id' }, 400)
    const tagIndex = state.tags.findIndex((tag) => tag.id === tagId)
    if (method === 'PUT') {
      if (tagIndex < 0) return jsonResponse({ error: 'Tag not found' }, 404)
      const payload = body as { name?: string; color?: string } | null
      const updated: Tag = {
        ...state.tags[tagIndex],
        name: payload?.name?.trim() || state.tags[tagIndex].name,
        color: payload?.color || state.tags[tagIndex].color,
      }
      state.tags[tagIndex] = updated
      return jsonResponse(updated)
    }
    if (method === 'DELETE') {
      state.tags = state.tags.filter((tag) => tag.id !== tagId)
      for (const frameTagIds of state.frameTags.values()) {
        frameTagIds.delete(tagId)
      }
      return jsonResponse({ ok: true })
    }
  }

  const frameTagsPathMatch = path.match(/^\/api\/frames\/(\d+)\/tags(?:\/(\d+))?$/)
  if (frameTagsPathMatch) {
    const frameId = parseId(frameTagsPathMatch[1])
    const tagId = parseId(frameTagsPathMatch[2])
    if (frameId == null) return jsonResponse({ error: 'Invalid frame id' }, 400)
    if (tagId == null && method === 'GET') {
      return jsonResponse(getFrameTagList(frameId))
    }
    if (tagId == null) {
      return jsonResponse({ error: 'Invalid tag id' }, 400)
    }
    if (method === 'POST') {
      ensureFrameTagSet(frameId).add(tagId)
      return jsonResponse({ ok: true }, 201)
    }
    if (method === 'DELETE') {
      ensureFrameTagSet(frameId).delete(tagId)
      return jsonResponse({ ok: true })
    }
  }

  if (path === '/api/reports' && method === 'GET') {
    return jsonResponse(makeDefaultReport())
  }
  if (path === '/api/backup' && method === 'GET') {
    return textBlobResponse(JSON.stringify(makeBackupArchive(), null, 2), 'application/json')
  }
  if (path === '/api/backup/restore' && method === 'POST') {
    return jsonResponse(makeRestoreResult())
  }
  if (path === '/api/timeline' && method === 'GET') {
    return jsonResponse(makeDefaultTimeline())
  }
  if (path === '/api/focus/metrics' && method === 'GET') {
    return jsonResponse(makeDefaultFocusMetrics())
  }
  if (path === '/api/focus/sessions' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/focus/interruptions' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/focus/suggestions' && method === 'GET') {
    return jsonResponse(state.suggestions)
  }

  if (path === '/api/tracking-schedule' && method === 'GET') {
    return jsonResponse(state.trackingSchedule)
  }
  if (path === '/api/tracking-schedule' && method === 'PUT') {
    state.trackingSchedule = (body ?? makeDefaultTrackingSchedule()) as TrackingScheduleConfig
    return jsonResponse(state.trackingSchedule)
  }
  if (path === '/api/tracking-schedule/status' && method === 'GET') {
    return jsonResponse(makeDefaultTrackingScheduleStatus())
  }

  const suggestionFeedbackMatch = path.match(/^\/api\/focus\/suggestions\/(\d+)\/feedback$/)
  if (suggestionFeedbackMatch && method === 'POST') {
    const suggestionId = parseId(suggestionFeedbackMatch[1])
    const action = (body as { action?: string } | null)?.action
    if (suggestionId != null && action) {
      state.suggestions = state.suggestions.map((suggestion) => {
        if (suggestion.id !== suggestionId) return suggestion
        const now = new Date().toISOString()
        if (action === 'shown') return { ...suggestion, shown_at: now }
        if (action === 'dismissed') return { ...suggestion, dismissed_at: now }
        if (action === 'acted') return { ...suggestion, acted_at: now }
        return suggestion
      })
    }
    return jsonResponse({ ok: true })
  }

  if (path === '/api/automation/status' && method === 'GET') {
    return jsonResponse(makeDefaultAutomationStatus())
  }
  if (path === '/api/automation/audit' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/automation/policies' && method === 'GET') {
    return jsonResponse(makeDefaultPolicies())
  }
  if (path === '/api/automation/execution-policies') {
    if (method === 'GET') {
      return jsonResponse(state.executionPolicies)
    }

    if (method === 'POST') {
      const policy = body as ExecutionPolicyConfig | null
      if (!policy?.policy_id) {
        return jsonResponse({ error: 'Policy ID is required' }, 400)
      }
      state.executionPolicies = [
        ...state.executionPolicies.filter((existing) => existing.policy_id !== policy.policy_id),
        policy,
      ]
      return jsonResponse(policy, 201)
    }
  }
  const executionPolicyMatch = path.match(/^\/api\/automation\/execution-policies\/([^/]+)$/)
  if (executionPolicyMatch) {
    const policyId = decodeURIComponent(executionPolicyMatch[1])

    if (method === 'PUT') {
      const policy = body as ExecutionPolicyConfig | null
      if (!policy?.policy_id) {
        return jsonResponse({ error: 'Policy ID is required' }, 400)
      }
      state.executionPolicies = state.executionPolicies.map((existing) =>
        existing.policy_id === policyId ? policy : existing,
      )
      if (!state.executionPolicies.some((existing) => existing.policy_id === policy.policy_id)) {
        state.executionPolicies.push(policy)
      }
      return jsonResponse(policy)
    }

    if (method === 'DELETE') {
      state.executionPolicies = state.executionPolicies.filter((existing) => existing.policy_id !== policyId)
      return new Response(null, { status: 204 })
    }
  }
  if (path === '/api/automation/contracts' && method === 'GET') {
    return jsonResponse({
      audit_schema_version: 'automation.audit.v1',
      scene_schema_version: 'ui_scene.v1',
      scene_action_schema_version: 'automation.scene_action.v1',
    })
  }
  if (path === '/api/automation/stats' && method === 'GET') {
    return jsonResponse(makeDefaultAutomationStats())
  }
  if (path === '/api/automation/scene' && method === 'GET') {
    const frameIdRaw = requestUrl.searchParams.get('frame_id')
    const frameId = frameIdRaw == null ? undefined : Number(frameIdRaw)
    const scene = makeDefaultAutomationScene(
      requestUrl.searchParams.get('app_name') ?? undefined,
      requestUrl.searchParams.get('screen_id') ?? undefined,
      Number.isFinite(frameId) ? frameId : undefined,
    )
    const cfg = state.settings.ai_provider.scene_intelligence
    if (!cfg.enabled) {
      return jsonResponse({ error: 'Scene intelligence is disabled' }, 400)
    }
    const filtered = scene.elements
      .filter((item) => item.confidence >= cfg.min_confidence)
      .sort((a, b) => b.confidence - a.confidence)
      .slice(0, cfg.max_elements)
    return jsonResponse({
      ...scene,
      elements: filtered,
    })
  }

  if (path === '/api/automation/scene/calibration' && method === 'GET') {
    const frameIdRaw = requestUrl.searchParams.get('frame_id')
    const frameId = frameIdRaw == null ? undefined : Number(frameIdRaw)
    const scene = makeDefaultAutomationScene(
      requestUrl.searchParams.get('app_name') ?? undefined,
      requestUrl.searchParams.get('screen_id') ?? undefined,
      Number.isFinite(frameId) ? frameId : undefined,
    )
    const cfg = state.settings.ai_provider.scene_intelligence
    if (!cfg.enabled) {
      return jsonResponse({ error: 'Scene intelligence is disabled' }, 400)
    }
    const filtered = scene.elements
      .filter((item) => Number.isFinite(item.confidence) && item.confidence >= cfg.min_confidence)
      .sort((a, b) => b.confidence - a.confidence)
      .slice(0, cfg.max_elements)
    const considered = filtered.filter((item) => Number.isFinite(item.confidence))
    const avgConfidence =
      considered.length > 0 ? considered.reduce((sum, item) => sum + item.confidence, 0) / considered.length : 0
    const reasons: string[] = []
    if (!cfg.calibration_enabled) {
      reasons.push('calibration disabled by configuration')
    } else {
      if (considered.length < cfg.calibration_min_elements) {
        reasons.push(`insufficient elements: ${considered.length} < ${cfg.calibration_min_elements}`)
      }
      if (avgConfidence < cfg.calibration_min_avg_confidence) {
        reasons.push(
          `low average confidence: ${avgConfidence.toFixed(3)} < ${cfg.calibration_min_avg_confidence.toFixed(3)}`,
        )
      }
    }
    return jsonResponse({
      schema_version: 'automation.scene_calibration.v1',
      scene_id: scene.scene_id,
      total_elements: filtered.length,
      considered_elements: considered.length,
      avg_confidence: avgConfidence,
      min_confidence: cfg.min_confidence,
      min_required_elements: cfg.calibration_min_elements,
      min_required_avg_confidence: cfg.calibration_min_avg_confidence,
      passed: cfg.calibration_enabled && reasons.length === 0,
      reasons,
    })
  }
  if (path === '/api/automation/presets' && method === 'GET') {
    return jsonResponse({ presets: state.presets })
  }
  if (path === '/api/automation/presets' && method === 'POST') {
    const payload = body as WorkflowPreset | null
    if (!payload) return jsonResponse({ error: 'Invalid preset' }, 400)
    state.presets = [...state.presets, payload]
    return jsonResponse(payload, 201)
  }

  const presetUpdateMatch = path.match(/^\/api\/automation\/presets\/([^/]+)$/)
  if (presetUpdateMatch && method === 'PUT') {
    const presetId = decodeURIComponent(presetUpdateMatch[1])
    const payload = body as WorkflowPreset | null
    if (!payload) return jsonResponse({ error: 'Invalid preset' }, 400)
    const existingIndex = state.presets.findIndex((preset) => preset.id === presetId)
    if (existingIndex < 0) return jsonResponse({ error: 'Preset not found' }, 404)
    const nextPreset = { ...payload, id: presetId }
    state.presets = state.presets.map((preset, index) => (index === existingIndex ? nextPreset : preset))
    return jsonResponse(nextPreset)
  }

  const presetDeleteMatch = path.match(/^\/api\/automation\/presets\/([^/]+)$/)
  if (presetDeleteMatch && method === 'DELETE') {
    const presetId = decodeURIComponent(presetDeleteMatch[1])
    state.presets = state.presets.filter((preset) => preset.id !== presetId)
    return jsonResponse({ ok: true })
  }

  const presetRunMatch = path.match(/^\/api\/automation\/presets\/([^/]+)\/run$/)
  if (presetRunMatch && method === 'POST') {
    const presetId = decodeURIComponent(presetRunMatch[1])
    const preset = state.presets.find((item) => item.id === presetId)
    const totalSteps = preset?.steps.length ?? 0
    return jsonResponse({
      preset_id: presetId,
      success: true,
      message: 'Standalone mode preset run simulated',
      steps_executed: totalSteps,
      total_steps: totalSteps,
      total_elapsed_ms: totalSteps * 10,
    })
  }

  if (path === '/api/automation/execute-hint' && method === 'POST') {
    const payload = body as { command_id?: string; session_id?: string; intent_hint?: string } | null
    const now = Date.now()
    const commandId = payload?.command_id?.trim() || `intent-hint-${now}`
    const sessionId = payload?.session_id?.trim() || 'standalone-session'
    const hint = payload?.intent_hint?.trim() || ''

    return jsonResponse({
      command_id: commandId,
      session_id: sessionId,
      planned_intent: {
        ClickElement: {
          text: hint || null,
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
    })
  }

  if (path === '/api/automation/execute-scene-action' && method === 'POST') {
    const payload = body as ExecuteSceneActionRequest | null
    const sceneCfg = state.settings.ai_provider.scene_intelligence
    if (!sceneCfg.enabled) {
      return jsonResponse({ error: 'Scene intelligence is disabled' }, 400)
    }
    if (!sceneCfg.allow_action_execution) {
      return jsonResponse({ error: 'Scene action execution is disabled' }, 400)
    }
    const now = Date.now()
    const commandId = payload?.command_id?.trim() || `scene-action-${now}`
    const sessionId = payload?.session_id?.trim() || 'standalone-session'
    return jsonResponse({
      schema_version: 'automation.scene_action.v1',
      command_id: commandId,
      session_id: sessionId,
      frame_id: payload?.frame_id,
      scene_id: payload?.scene_id,
      element_id: payload?.element_id ?? 'unknown-element',
      applied_privacy_policy: 'AllowFiltered',
      scene_action_override_active: false,
      scene_action_override_expires_at: null,
      executed_intents:
        payload?.action_type === 'type_text'
          ? [
              { Raw: { MouseClick: { button: 'left', x: 0, y: 0 } } },
              { Raw: { KeyType: { text: payload?.text ?? '' } } },
            ]
          : [{ Raw: { MouseClick: { button: 'left', x: 0, y: 0 } } }],
      result: {
        success: true,
        element: null,
        verification: null,
        retry_count: 0,
        elapsed_ms: 0,
        error: null,
      },
    })
  }

  // ── Coaching endpoints ────────────────────────────────────────
  if (path === '/api/coaching/history' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/coaching/goals' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/coaching/goals' && method === 'PUT') {
    return jsonResponse({ ok: true })
  }

  // ── Recalibration overrides ─────────────────────────────────
  if (path === '/api/recalibration/overrides' && method === 'GET') {
    return jsonResponse([])
  }

  // ── Pomodoro status ─────────────────────────────────────────
  if (path === '/api/pomodoro/current' && method === 'GET') {
    return jsonResponse(null)
  }

  // ── Dashboard Day ───────────────────────────────────────────
  if (path === '/api/dashboard/day' && method === 'GET') {
    return jsonResponse({
      date: todayIsoDate(),
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
        comparison: null,
      },
    })
  }

  // ── GUI Heatmap ─────────────────────────────────────────────
  if (path === '/api/stats/gui-heatmap' && method === 'GET') {
    return jsonResponse([])
  }

  // ── Digests ─────────────────────────────────────────────────
  if (path === '/api/digests' && method === 'GET') {
    return jsonResponse([])
  }
  if (path === '/api/digests/current' && method === 'GET') {
    return jsonResponse(null)
  }

  // ── Semantic Search ─────────────────────────────────────────
  if (path === '/api/semantic-search' && method === 'GET') {
    return jsonResponse({ results: [] })
  }

  // ── Onboarding ──────────────────────────────────────────────
  if (path === '/api/onboarding/quickstart' && method === 'GET') {
    return jsonResponse({
      schema_version: 'onboarding.quickstart.v1',
      generated_at: new Date().toISOString(),
      target_mode: 'standalone',
      dashboard_url: '/',
      checklist: [],
      recommended_presets: [],
      verification_commands: [],
    })
  }

  // ── Support Diagnostics ─────────────────────────────────────
  if (path === '/api/support/diagnostics' && method === 'GET') {
    return jsonResponse({
      schema_version: 'support.diagnostics.v1',
      generated_at: new Date().toISOString(),
      health: {
        storage_ok: true,
        storage_error: null,
        frames_dir_configured: false,
        frames_dir_path: null,
        frames_dir_exists: null,
        config_manager_configured: false,
        automation_controller_configured: false,
        update_control_configured: false,
      },
      settings_snapshot: makeDefaultSettings(),
      storage_stats: makeDefaultStorageStats(),
      recent_audit_entries: [],
      recent_policy_events: [],
    })
  }

  // ── Playbook Library ────────────────────────────────────────
  if (path === '/api/playbooks/coaching' && method === 'GET') {
    return jsonResponse({
      templates: [
        {
          profile: 'deep_work',
          trigger_type: 'session_start',
          tone: 'motivating',
          locale: 'en',
          text: "Let's lock in and get some deep work done. Minimize distractions and focus on your top priority.",
        },
        {
          profile: 'deep_work',
          trigger_type: 'session_start',
          tone: 'motivating',
          locale: 'ko',
          text: '집중 작업 세션을 시작합니다. 방해 요소를 최소화하고 최우선 과제에 집중하세요.',
        },
        {
          profile: 'balanced',
          trigger_type: 'idle_return',
          tone: 'gentle',
          locale: 'en',
          text: 'Welcome back! You were idle for a bit. Ready to ease back in?',
        },
        {
          profile: 'balanced',
          trigger_type: 'idle_return',
          tone: 'gentle',
          locale: 'ko',
          text: '돌아오셨군요! 잠시 자리를 비웠네요. 다시 시작할 준비가 되셨나요?',
        },
        {
          profile: 'flow_seeker',
          trigger_type: 'context_switch',
          tone: 'curious',
          locale: 'en',
          text: 'You just switched context. Take a moment to reset and re-engage with your new task.',
        },
      ],
    })
  }
  if (path === '/api/playbooks/presets' && method === 'GET') {
    return jsonResponse({
      presets: [
        {
          id: 'focus-mode',
          name: 'Focus Mode',
          description: 'Closes distracting apps and enables DND',
          category: 'Productivity',
          step_count: 3,
          builtin: true,
        },
        {
          id: 'end-of-day',
          name: 'End of Day',
          description: 'Saves session summary and closes all work apps',
          category: 'Workflow',
          step_count: 5,
          builtin: true,
        },
        {
          id: 'meeting-prep',
          name: 'Meeting Prep',
          description: 'Opens calendar, notes, and mutes notifications',
          category: 'Productivity',
          step_count: 4,
          builtin: true,
        },
        {
          id: 'app-cleanup',
          name: 'App Cleanup',
          description: 'Closes unused background applications',
          category: 'AppManagement',
          step_count: 2,
          builtin: true,
        },
        {
          id: 'custom-workflow',
          name: 'My Workflow',
          description: 'Custom personal automation workflow',
          category: 'Custom',
          step_count: 6,
          builtin: false,
        },
      ],
    })
  }

  // Unknown /api route fallback: avoid hard failures in standalone mode.
  return jsonResponse({ ok: true })
}
