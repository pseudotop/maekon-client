import { describe, expect, it } from 'vitest'
import type { ReportResponse } from '../../api/client'
import { ApiClientError } from '../../api/client'
import { requiresCaptureReauthForExport } from '../../api/reauth'
import {
  buildReportExportFilename,
  buildReportSummaryFilename,
  buildSessionExportFilename,
  serializeReportSummary,
} from './ExportSection'

// #8081-A: /reports/export is now a real export surface. These guard the
// download filenames (scope-encoded, non-colliding) and the report-summary
// serialization (an exact snapshot of the loaded report).
describe('reports export helpers (#8081-A)', () => {
  it('encodes data type, date range, and format into the raw-export filename', () => {
    expect(buildReportExportFilename('metrics', 'csv', '2026-07-01', '2026-07-08')).toBe(
      'maekon_report_metrics_2026-07-01_2026-07-08.csv',
    )
    expect(buildReportExportFilename('events', 'json', '2026-07-01', '2026-07-08')).toBe(
      'maekon_report_events_2026-07-01_2026-07-08.json',
    )
  })

  it('omits the range suffix when no dates are supplied', () => {
    expect(buildReportExportFilename('frames', 'json')).toBe('maekon_report_frames.json')
    expect(buildReportSummaryFilename()).toBe('maekon_report_summary.json')
  })

  it('always names the report-summary export .json', () => {
    expect(buildReportSummaryFilename('2026-07-01', '2026-07-08')).toBe(
      'maekon_report_summary_2026-07-01_2026-07-08.json',
    )
  })

  it('serializes the loaded report verbatim (round-trips to the same object)', () => {
    const report = {
      title: 'Weekly',
      from_date: '2026-07-01',
      to_date: '2026-07-08',
      days: 7,
      total_active_secs: 100,
      total_idle_secs: 20,
      total_captures: 5,
      total_events: 42,
      avg_cpu: 12.5,
      avg_memory: 34.1,
      daily_stats: [],
      app_stats: [],
      hourly_activity: [],
      productivity: {},
    } as unknown as ReportResponse

    const serialized = serializeReportSummary(report)
    expect(JSON.parse(serialized)).toEqual(report)
    // Pretty-printed for human readability.
    expect(serialized).toContain('\n')
  })

  // #9854: the session-interchange exports carry a FIXED extension each,
  // independent of the JSON/CSV toggle. A shared extension would hand a
  // calendar importer a `.csv` (or Toggl a `.ics`) and fail at the receiver.
  it('names session exports with the extension the receiving tool requires', () => {
    expect(buildSessionExportFilename('ical', '2026-07-01', '2026-07-08')).toBe(
      'maekon_sessions_ical_2026-07-01_2026-07-08.ics',
    )
    expect(buildSessionExportFilename('toggl', '2026-07-01', '2026-07-08')).toBe(
      'maekon_sessions_toggl_2026-07-01_2026-07-08.csv',
    )
  })

  it('omits the range suffix from session exports when no dates are supplied', () => {
    expect(buildSessionExportFilename('ical')).toBe('maekon_sessions_ical.ics')
    expect(buildSessionExportFilename('toggl')).toBe('maekon_sessions_toggl.csv')
  })

  it('never treats a session export as capture-protected', () => {
    // These export session start/end times and titles, never frames, so the
    // capture re-auth gate must not fire for them.
    const locked = new ApiClientError('auth.reauth_required', 'Re-authentication required', 403)
    expect(requiresCaptureReauthForExport('ical' as never, locked)).toBe(false)
    expect(requiresCaptureReauthForExport('toggl' as never, locked)).toBe(false)
  })

  it('requests capture re-auth only for a protected frame export', () => {
    const locked = new ApiClientError('auth.reauth_required', 'Re-authentication required', 403)

    expect(requiresCaptureReauthForExport('frames', locked)).toBe(true)
    expect(requiresCaptureReauthForExport('events', locked)).toBe(false)
    expect(requiresCaptureReauthForExport('metrics', locked)).toBe(false)
    expect(requiresCaptureReauthForExport('frames', new Error('network unavailable'))).toBe(false)
  })
})
