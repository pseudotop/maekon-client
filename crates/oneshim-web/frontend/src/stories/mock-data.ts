// Factory functions for generating realistic mock data in page stories.
// Each factory returns sensible defaults and accepts optional partial overrides.

import type { CoachingEvent, GoalProgress } from '../api/coaching'
import type {
  AppSegment,
  AppUsage,
  DailyDigestResponse,
  DailyDigestSegment,
  DailyDigestStatistics,
  DailySummary,
  DesktopPermissionSnapshot,
  FocusMetrics,
  FocusMetricsResponse,
  Frame,
  GuiHeatmapCell,
  HeatmapResponse,
  HourlyMetrics,
  Interruption,
  PaginatedResponse,
  ProcessEntry,
  ProcessSnapshot,
  ReportAppStat,
  ReportDailyStat,
  ReportHourlyActivity,
  ReportResponse,
  StorageStats,
  Tag,
  TimelineItem,
  TimelineResponse,
  TimelineSessionInfo,
  UpdateStatus,
  WorkSession,
} from '../api/contracts'

// ── Helpers ─────────────────────────────────────────────────────

function isoDate(daysAgo = 0): string {
  const d = new Date()
  d.setDate(d.getDate() - daysAgo)
  return d.toISOString().split('T')[0]
}

function isoTimestamp(hoursAgo = 0): string {
  const d = new Date()
  d.setHours(d.getHours() - hoursAgo)
  return d.toISOString()
}

const APP_NAMES = ['Visual Studio Code', 'Google Chrome', 'Slack', 'Terminal', 'Finder', 'Safari', 'Notes', 'Figma']
const TAG_COLORS = ['#3b82f6', '#ef4444', '#22c55e', '#f59e0b', '#8b5cf6', '#ec4899']
const TRIGGER_TYPES = ['timer', 'window_change', 'user_action', 'smart']

// ── AppUsage ────────────────────────────────────────────────────

export function createMockAppUsage(count = 5): AppUsage[] {
  return APP_NAMES.slice(0, count).map((name, i) => ({
    name,
    duration_secs: Math.round(3600 * (count - i) * (0.8 + Math.random() * 0.4)),
    event_count: Math.round(120 * (count - i) * (0.6 + Math.random() * 0.8)),
    frame_count: Math.round(40 * (count - i) * (0.5 + Math.random() * 1.0)),
  }))
}

// ── DailySummary ────────────────────────────────────────────────

export function createMockSummary(overrides?: Partial<DailySummary>): DailySummary {
  const topApps = createMockAppUsage(4)
  return {
    date: isoDate(),
    total_active_secs: 21600,
    total_idle_secs: 5400,
    top_apps: topApps,
    cpu_avg: 34.2,
    memory_avg_percent: 61.8,
    frames_captured: 482,
    events_logged: 1537,
    ...overrides,
  }
}

// ── HourlyMetrics ───────────────────────────────────────────────

export function createMockHourlyMetrics(count = 24): HourlyMetrics[] {
  return Array.from({ length: count }, (_, i) => {
    const hour = String(i).padStart(2, '0')
    const cpuBase = i >= 9 && i <= 18 ? 30 + Math.random() * 40 : 5 + Math.random() * 15
    const memBase = 55 + Math.random() * 20
    return {
      hour: `${hour}:00`,
      cpu_avg: Math.round(cpuBase * 10) / 10,
      cpu_max: Math.round((cpuBase + 15 + Math.random() * 20) * 10) / 10,
      memory_avg: Math.round(memBase * 10) / 10,
      memory_max: Math.round((memBase + 5 + Math.random() * 10) * 10) / 10,
      sample_count: i >= 9 && i <= 18 ? 60 : Math.round(Math.random() * 20),
    }
  })
}

// ── ProcessEntry / ProcessSnapshot ──────────────────────────────

export function createMockProcesses(count = 5): ProcessEntry[] {
  const names = ['code', 'chrome', 'node', 'slack', 'dockerd', 'rust-analyzer', 'cargo', 'safari']
  return names.slice(0, count).map((name, i) => ({
    pid: 1000 + i * 137,
    name,
    cpu_usage: Math.round((12 - i * 1.5 + Math.random() * 5) * 10) / 10,
    memory_bytes: Math.round((512 - i * 60 + Math.random() * 100) * 1024 * 1024),
  }))
}

export function createMockProcessSnapshot(overrides?: Partial<ProcessSnapshot>): ProcessSnapshot {
  return {
    timestamp: isoTimestamp(),
    processes: createMockProcesses(5),
    ...overrides,
  }
}

// ── StorageStats ───────────────────────────────────────────────

export function createMockStorageStats(overrides?: Partial<StorageStats>): StorageStats {
  return {
    db_size_bytes: 128 * 1024 * 1024,
    frames_size_bytes: 392 * 1024 * 1024,
    total_size_bytes: 520 * 1024 * 1024,
    frame_count: 12480,
    event_count: 44820,
    metric_count: 181200,
    oldest_data_date: new Date(Date.now() - 35 * 24 * 3600 * 1000).toISOString(),
    newest_data_date: isoTimestamp(),
    ...overrides,
  }
}

// ── Frame / PaginatedResponse<Frame> ────────────────────────────

export function createMockFrame(id: number, overrides?: Partial<Frame>): Frame {
  const appIdx = id % APP_NAMES.length
  return {
    id,
    timestamp: isoTimestamp(id * 0.1),
    trigger_type: TRIGGER_TYPES[id % TRIGGER_TYPES.length],
    app_name: APP_NAMES[appIdx],
    window_title: `${APP_NAMES[appIdx]} - Project workspace`,
    importance: Math.round((0.3 + Math.random() * 0.7) * 100) / 100,
    resolution: '1920x1080',
    file_path: null,
    ocr_text: id % 3 === 0 ? 'Sample OCR text extracted from the screen capture.' : null,
    image_url: `/api/frames/${id}/image`,
    tag_ids: id % 4 === 0 ? [1] : [],
    ...overrides,
  }
}

export function createMockFrames(count = 12): Frame[] {
  return Array.from({ length: count }, (_, i) => createMockFrame(i + 1))
}

export function createMockPaginatedFrames(count = 12, total?: number, offset = 0): PaginatedResponse<Frame> {
  const resolvedTotal = total ?? count
  return {
    data: createMockFrames(count),
    pagination: {
      total: resolvedTotal,
      offset,
      limit: count,
      has_more: offset + count < resolvedTotal,
    },
  }
}

// ── Tag ─────────────────────────────────────────────────────────

export function createMockTags(count = 4): Tag[] {
  const names = ['Work', 'Bug', 'Feature', 'Design', 'Meeting', 'Review']
  return names.slice(0, count).map((name, i) => ({
    id: i + 1,
    name,
    color: TAG_COLORS[i % TAG_COLORS.length],
    created_at: isoTimestamp(24 * (count - i)),
  }))
}

// ── ReportResponse ──────────────────────────────────────────────

function createMockDailyStats(days: number): ReportDailyStat[] {
  return Array.from({ length: days }, (_, i) => ({
    date: isoDate(days - 1 - i),
    active_secs: 18000 + Math.round(Math.random() * 10800),
    idle_secs: 3600 + Math.round(Math.random() * 5400),
    captures: 300 + Math.round(Math.random() * 400),
    events: 1000 + Math.round(Math.random() * 1500),
    cpu_avg: Math.round((25 + Math.random() * 30) * 10) / 10,
    memory_avg: Math.round((55 + Math.random() * 20) * 10) / 10,
  }))
}

function createMockAppStats(): ReportAppStat[] {
  const apps = createMockAppUsage(5)
  const totalDuration = apps.reduce((sum, a) => sum + a.duration_secs, 0)
  return apps.map((a) => ({
    name: a.name,
    duration_secs: a.duration_secs,
    events: a.event_count,
    captures: a.frame_count,
    percentage: Math.round((a.duration_secs / totalDuration) * 1000) / 10,
  }))
}

function createMockHourlyActivity(): ReportHourlyActivity[] {
  return Array.from({ length: 24 }, (_, hour) => ({
    hour,
    activity: hour >= 9 && hour <= 18 ? 60 + Math.round(Math.random() * 40) : Math.round(Math.random() * 20),
  }))
}

export function createMockReport(overrides?: Partial<ReportResponse>): ReportResponse {
  const days = 7
  const dailyStats = createMockDailyStats(days)
  const totalActive = dailyStats.reduce((s, d) => s + d.active_secs, 0)
  const totalIdle = dailyStats.reduce((s, d) => s + d.idle_secs, 0)
  const totalCaptures = dailyStats.reduce((s, d) => s + d.captures, 0)
  const totalEvents = dailyStats.reduce((s, d) => s + d.events, 0)
  const avgCpu = dailyStats.reduce((s, d) => s + d.cpu_avg, 0) / days
  const avgMemory = dailyStats.reduce((s, d) => s + d.memory_avg, 0) / days

  return {
    title: 'Weekly Activity Report',
    from_date: isoDate(days - 1),
    to_date: isoDate(),
    days,
    total_active_secs: totalActive,
    total_idle_secs: totalIdle,
    total_captures: totalCaptures,
    total_events: totalEvents,
    avg_cpu: Math.round(avgCpu * 10) / 10,
    avg_memory: Math.round(avgMemory * 10) / 10,
    daily_stats: dailyStats,
    app_stats: createMockAppStats(),
    hourly_activity: createMockHourlyActivity(),
    productivity: {
      score: 72,
      active_ratio: Math.round((totalActive / (totalActive + totalIdle)) * 1000) / 10,
      peak_hour: 14,
      top_app: 'Visual Studio Code',
      trend: 5.3,
    },
    ...overrides,
  }
}

// ── UpdateStatus ───────────────────────────────────────────────

export function createMockUpdateStatus(overrides?: Partial<UpdateStatus>): UpdateStatus {
  return {
    enabled: true,
    auto_install: false,
    phase: 'PendingApproval',
    message: 'A new release is ready to install.',
    pending: {
      current_version: '0.4.9',
      latest_version: '0.4.10',
      release_url: 'https://github.com/pseudotop/oneshim-client/releases/tag/v0.4.10',
      release_name: 'v0.4.10',
      published_at: new Date(Date.now() - 2 * 24 * 3600 * 1000).toISOString(),
      download_url: 'https://github.com/pseudotop/oneshim-client/releases/download/v0.4.10/oneshim.dmg',
    },
    revision: 4,
    updated_at: isoTimestamp(),
    ...overrides,
  }
}

// ── FocusMetrics ────────────────────────────────────────────────

export function createMockFocusMetrics(overrides?: Partial<FocusMetrics>): FocusMetrics {
  return {
    date: isoDate(),
    total_active_secs: 21600,
    deep_work_secs: 14400,
    communication_secs: 3600,
    context_switches: 12,
    interruption_count: 5,
    avg_focus_duration_secs: 2400,
    max_focus_duration_secs: 5400,
    focus_score: 72,
    ...overrides,
  }
}

export function createMockFocusMetricsResponse(overrides?: Partial<FocusMetricsResponse>): FocusMetricsResponse {
  return {
    today: createMockFocusMetrics(),
    history: Array.from({ length: 7 }, (_, i) =>
      createMockFocusMetrics({
        date: isoDate(6 - i),
        focus_score: 55 + Math.round(Math.random() * 30),
        deep_work_secs: 10800 + Math.round(Math.random() * 7200),
        communication_secs: 1800 + Math.round(Math.random() * 3600),
      }),
    ),
    ...overrides,
  }
}

// ── Heatmap / GUI Heatmap ──────────────────────────────────────

export function createMockHeatmapResponse(days = 7, overrides?: Partial<HeatmapResponse>): HeatmapResponse {
  const fromDate = new Date()
  fromDate.setDate(fromDate.getDate() - (days - 1))

  const cells = Array.from({ length: days }, (_, day) =>
    Array.from({ length: 24 }, (_, hour) => ({
      day,
      hour,
      value: hour >= 8 && hour <= 18 ? Math.round(15 + Math.random() * 70) : Math.round(Math.random() * 10),
    })),
  ).flat()

  const maxValue = Math.max(...cells.map((cell) => cell.value), 1)

  return {
    from_date: fromDate.toISOString().split('T')[0],
    to_date: isoDate(),
    cells,
    max_value: maxValue,
    ...overrides,
  }
}

export function createMockGuiHeatmapCells(count = 24): GuiHeatmapCell[] {
  return Array.from({ length: count }, (_, i) => ({
    hour: `${String(i).padStart(2, '0')}:00`,
    count: i >= 9 && i <= 18 ? Math.floor(18 + Math.random() * 64) : Math.floor(Math.random() * 8),
  }))
}

// ── Desktop Permission Snapshot ────────────────────────────────

export function createMockDesktopPermissionSnapshot(
  overrides?: Partial<DesktopPermissionSnapshot>,
): DesktopPermissionSnapshot {
  return {
    platform: 'macos',
    accessibility: {
      state: 'granted',
      status_reason: null,
    },
    screen_capture: {
      state: 'needs_attention',
      status_reason: 'macos_screen_capture_missing',
    },
    notifications: {
      state: 'granted',
      status_reason: 'macos_notifications_granted',
    },
    ...overrides,
  }
}

// ── WorkSession ─────────────────────────────────────────────────

export function createMockWorkSessions(count = 5): WorkSession[] {
  const categories = ['Development', 'Communication', 'Documentation', 'Browser', 'Design']
  return Array.from({ length: count }, (_, i) => ({
    id: i + 1,
    started_at: isoTimestamp(count - i + 1),
    ended_at: i === 0 ? null : isoTimestamp(count - i),
    primary_app: APP_NAMES[i % APP_NAMES.length],
    category: categories[i % categories.length],
    state: i === 0 ? 'active' : 'completed',
    interruption_count: Math.round(Math.random() * 4),
    deep_work_secs: 1200 + Math.round(Math.random() * 3600),
    duration_secs: 1800 + Math.round(Math.random() * 5400),
  }))
}

// ── Interruption ────────────────────────────────────────────────

export function createMockInterruptions(count = 5): Interruption[] {
  return Array.from({ length: count }, (_, i) => ({
    id: i + 1,
    interrupted_at: isoTimestamp(count - i),
    from_app: APP_NAMES[i % APP_NAMES.length],
    from_category: 'Development',
    to_app: APP_NAMES[(i + 1) % APP_NAMES.length],
    to_category: 'Communication',
    resumed_at: i < count - 1 ? isoTimestamp(count - i - 0.5) : null,
    resumed_to_app: i < count - 1 ? APP_NAMES[i % APP_NAMES.length] : null,
    duration_secs: 60 + Math.round(Math.random() * 300),
  }))
}

// ── DailyDigestResponse ─────────────────────────────────────────

export function createMockDailyDigest(overrides?: Partial<DailyDigestResponse>): DailyDigestResponse {
  const segments: DailyDigestSegment[] = [
    {
      segment_id: 'seg-1',
      start_time: `${isoDate()}T09:00:00Z`,
      end_time: `${isoDate()}T10:30:00Z`,
      duration_mins: 90,
      regime_label: 'Deep Work',
      regime_color: '#3b82f6',
      regime_id: 'deep-work',
      dominant_app: 'Visual Studio Code',
      content_summary: [{ content: 'Refactored authentication module', work_type: 'coding', mins: 90 }],
    },
    {
      segment_id: 'seg-2',
      start_time: `${isoDate()}T10:30:00Z`,
      end_time: `${isoDate()}T11:00:00Z`,
      duration_mins: 30,
      regime_label: 'Communication',
      regime_color: '#22c55e',
      regime_id: 'communication',
      dominant_app: 'Slack',
      content_summary: [{ content: 'Team standup discussion', work_type: 'communication', mins: 30 }],
    },
    {
      segment_id: 'seg-3',
      start_time: `${isoDate()}T11:00:00Z`,
      end_time: `${isoDate()}T12:30:00Z`,
      duration_mins: 90,
      regime_label: 'Deep Work',
      regime_color: '#3b82f6',
      regime_id: 'deep-work',
      dominant_app: 'Visual Studio Code',
      content_summary: [{ content: 'Implemented new dashboard features', work_type: 'coding', mins: 90 }],
    },
  ]

  const statistics: DailyDigestStatistics = {
    deep_work_hours: 3.0,
    communication_hours: 0.5,
    meeting_hours: 0,
    context_switches: 8,
    longest_focus_mins: 90,
    longest_focus_content: 'Refactored authentication module',
    regime_distribution: { 'deep-work': 180, communication: 30 },
  }

  return {
    date: isoDate(),
    insight: {
      narrative: 'You had a productive morning with 3 hours of deep work and minimal interruptions.',
      highlights: [
        { highlight_type: 'achievement', text: '3 hours of focused coding' },
        { highlight_type: 'streak', text: '90-minute unbroken focus session' },
      ],
    },
    timeline: segments,
    statistics,
    ...overrides,
  }
}

// ── CoachingEvent / GoalProgress ────────────────────────────────

export function createMockCoachingHistory(count = 5): CoachingEvent[] {
  const triggers = ['regime_change', 'time_goal', 'idle_detected', 'focus_break', 'daily_summary']
  const profiles = ['Focus Coach', 'Break Reminder', 'Goal Tracker']
  const messages = [
    "You've been in deep focus for 90 minutes. Consider a short break.",
    'Great job! You completed your deep work goal for today.',
    'You switched contexts 5 times in the last hour. Try batching similar tasks.',
    'Idle detected. Would you like to resume your previous task?',
    'Daily summary: 4h deep work, 1h communication, 8 context switches.',
  ]

  return Array.from({ length: count }, (_, i) => ({
    event_id: `evt-${i + 1}`,
    trigger_type: triggers[i % triggers.length],
    profile_name: profiles[i % profiles.length],
    regime_id: i % 2 === 0 ? 'deep-work' : null,
    message_template: messages[i % messages.length],
    personalized_message: i % 3 === 0 ? messages[i % messages.length] : null,
    shown_at: isoTimestamp(count - i),
    dismissed_at: i % 2 === 0 ? isoTimestamp(count - i - 0.1) : null,
    dismiss_action: i % 2 === 0 ? 'acknowledged' : null,
    feedback_type: i % 3 === 0 ? 'helpful' : null,
    feedback_score: i % 3 === 0 ? 4 : null,
  }))
}

export function createMockGoalProgress(count = 4): GoalProgress[] {
  const goals = [
    { regime_label: 'Deep Work', target_minutes: 240, display_color: '#3b82f6' },
    { regime_label: 'Communication', target_minutes: 60, display_color: '#22c55e' },
    { regime_label: 'Meeting', target_minutes: 90, display_color: '#f59e0b' },
    { regime_label: 'Break', target_minutes: 30, display_color: '#8b5cf6' },
  ]

  return goals.slice(0, count).map((g) => {
    const current = Math.round(g.target_minutes * (0.3 + Math.random() * 0.7))
    return {
      regime_label: g.regime_label,
      current_minutes: current,
      target_minutes: g.target_minutes,
      percentage: Math.round((current / g.target_minutes) * 100),
      display_color: g.display_color,
    }
  })
}

// ── TimelineResponse (SessionReplay) ────────────────────────────

export function createMockTimelineResponse(overrides?: Partial<TimelineResponse>): TimelineResponse {
  const now = new Date()
  const startTime = new Date(now.getTime() - 3600 * 1000) // 1 hour ago

  const session: TimelineSessionInfo = {
    start: startTime.toISOString(),
    end: now.toISOString(),
    duration_secs: 3600,
    total_events: 45,
    total_frames: 12,
    total_idle_secs: 300,
  }

  const items: TimelineItem[] = Array.from({ length: 12 }, (_, i) => {
    const timestamp = new Date(startTime.getTime() + i * 300 * 1000).toISOString()
    if (i % 4 === 0) {
      return {
        type: 'Event' as const,
        id: `evt-${i}`,
        timestamp,
        event_type: 'window_change',
        app_name: APP_NAMES[i % APP_NAMES.length],
        window_title: `${APP_NAMES[i % APP_NAMES.length]} - workspace`,
      }
    }
    if (i % 6 === 0) {
      return {
        type: 'IdlePeriod' as const,
        start: timestamp,
        end: new Date(new Date(timestamp).getTime() + 120 * 1000).toISOString(),
        duration_secs: 120,
      }
    }
    return {
      type: 'Frame' as const,
      id: i + 1,
      timestamp,
      app_name: APP_NAMES[i % APP_NAMES.length],
      window_title: `${APP_NAMES[i % APP_NAMES.length]} - Project workspace`,
      importance: 0.3 + Math.random() * 0.7,
      image_url: `/api/frames/${i + 1}/image`,
    }
  })

  const segments: AppSegment[] = APP_NAMES.slice(0, 3).map((name, i) => ({
    app_name: name,
    start: new Date(startTime.getTime() + i * 1200 * 1000).toISOString(),
    end: new Date(startTime.getTime() + (i + 1) * 1200 * 1000).toISOString(),
    color: TAG_COLORS[i % TAG_COLORS.length],
  }))

  return {
    session,
    items,
    segments,
    ...overrides,
  }
}
