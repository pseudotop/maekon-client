import { screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../__tests__/helpers/render-helpers'
import { fetchCurrentDigest, fetchSummary } from '../api/client'
import type { DailySummary, WeeklyDigest } from '../api/contracts'
import DashboardWeek from './DashboardWeek'

vi.mock('../api/client', () => ({
  fetchCurrentDigest: vi.fn(),
  fetchSummary: vi.fn(),
}))

const RAW_TODAY_SUMMARY: DailySummary = {
  date: '2026-07-12',
  total_active_secs: 5400,
  total_idle_secs: 120,
  top_apps: [],
  cpu_avg: 25,
  memory_avg_percent: 50,
  frames_captured: 503,
  events_logged: 1028,
}

/** Exact Rust wire shape (weekly_digest.rs, no serde renames) — this fixture
 *  pins the #5676 contract fix: the previous hand-written TS type
 *  (total_minutes / category / deep_work_delta) deserialized these fields as
 *  undefined and the page would have rendered NaN/blank. */
const WIRE_DIGEST: WeeklyDigest = {
  week_start: '2026-06-01T00:00:00Z',
  week_end: '2026-06-08T00:00:00Z',
  total_tracked_hours: 32.5,
  regime_breakdown: { 'Deep Work': 18.2, Communication: 6.3 },
  category_breakdown: { coding: 20.1, email: 4.4 },
  top_content: [{ content_label: 'auth-module', total_mins: 340, dominant_work_type: 'ACTIVE_CODING' }],
  deep_work_hours: 18.2,
  communication_hours: 6.3,
  context_switches_total: 87,
  longest_deep_work_segment_mins: 95,
  comparison: {
    deep_work_delta_hours: 2.4,
    communication_delta_hours: -1.1,
    context_switch_delta: -12,
    trend_summary: 'More deep work, fewer interruptions than last week.',
  },
  llm_narrative: null,
}

describe('DashboardWeek page (#5676)', () => {
  beforeEach(() => {
    vi.mocked(fetchCurrentDigest).mockResolvedValue(WIRE_DIGEST)
    vi.mocked(fetchSummary).mockResolvedValue({
      ...RAW_TODAY_SUMMARY,
      total_active_secs: 0,
      frames_captured: 0,
      events_logged: 0,
    })
  })

  it('renders wire-shape digest fields (pins the TS contract fix)', async () => {
    renderWithProviders(<DashboardWeek />)

    // Stat tiles from top-level fields.
    expect(await screen.findByText('32.5h')).toBeInTheDocument()
    expect(screen.getByText('87')).toBeInTheDocument()
    expect(screen.getByText('95m')).toBeInTheDocument()

    // top_content uses total_mins + dominant_work_type (NOT the drifted
    // total_minutes / category names).
    expect(screen.getByText('auth-module')).toBeInTheDocument()
    expect(screen.getByText('340m')).toBeInTheDocument()
    expect(screen.getByText('Active Coding')).toBeInTheDocument()

    // comparison uses *_delta_hours + trend_summary.
    expect(screen.getByText('+2.4h')).toBeInTheDocument()
    expect(screen.getByText('-1.1h')).toBeInTheDocument()
    expect(screen.getByText('More deep work, fewer interruptions than last week.')).toBeInTheDocument()

    // llm_narrative is null until a weekly narrative producer exists —
    // the page must say so instead of showing a blank card.
    expect(screen.getByText('Narrative unavailable.')).toBeInTheDocument()
  })

  it('shows the empty state when no digest exists yet', async () => {
    vi.mocked(fetchCurrentDigest).mockResolvedValueOnce(null)

    renderWithProviders(<DashboardWeek />)

    expect(
      await screen.findByText(
        'No weekly digest yet. One is generated automatically each week once activity segments accumulate.',
      ),
    ).toBeInTheDocument()
  })

  it('labels a stale digest as the last available week', async () => {
    // week_end far in the past → /digests/current returned an old row.
    vi.mocked(fetchCurrentDigest).mockResolvedValueOnce({
      ...WIRE_DIGEST,
      week_start: '2026-01-05T00:00:00Z',
      week_end: '2026-01-12T00:00:00Z',
    })

    renderWithProviders(<DashboardWeek />)

    expect(await screen.findByText('Last available week')).toBeInTheDocument()
  })

  it('reconciles raw capture totals while the weekly digest is still pending', async () => {
    vi.mocked(fetchCurrentDigest).mockResolvedValueOnce({
      ...WIRE_DIGEST,
      total_tracked_hours: 0,
      deep_work_hours: 0,
      communication_hours: 0,
      context_switches_total: 0,
      longest_deep_work_segment_mins: 0,
    })
    vi.mocked(fetchSummary).mockResolvedValueOnce(RAW_TODAY_SUMMARY)

    renderWithProviders(<DashboardWeek />)

    expect(await screen.findByText('Captured activity is awaiting analysis')).toBeInTheDocument()
    expect(screen.getByText('1.5h')).toBeInTheDocument()
    expect(screen.getByText('503')).toBeInTheDocument()
    expect(screen.getByText('1,028')).toBeInTheDocument()
  })
})
