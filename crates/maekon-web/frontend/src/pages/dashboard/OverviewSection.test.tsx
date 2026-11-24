import { act, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { Outlet, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { fetchDailyDigest } from '../../api/client'
import type { DailyDigestResponse, DailySummary } from '../../api/contracts'
import type { CaptureStatus } from '../../hooks/useCaptureStatus'
import i18n from '../../i18n'
import { formatDuration } from '../../utils/formatters'
import type { DashboardContext } from './DashboardLayout'
import OverviewSection from './OverviewSection'

const captureStatus = vi.hoisted(() => ({ current: null as CaptureStatus | null }))

vi.mock('../../hooks/useCaptureStatus', () => ({ useCaptureStatus: () => captureStatus.current }))

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>()
  return { ...actual, fetchDailyDigest: vi.fn() }
})

const EMPTY_SUMMARY: DailySummary = {
  date: '2026-11-10',
  total_active_secs: 0,
  total_idle_secs: 0,
  top_apps: [],
  cpu_avg: 0,
  memory_avg_percent: 0,
  frames_captured: 0,
  events_logged: 0,
}

const ACTIVE_SUMMARY: DailySummary = {
  ...EMPTY_SUMMARY,
  total_active_secs: 22_020, // 6h 7m 0s — the long format must not appear
  total_idle_secs: 1_800,
  top_apps: [
    { name: 'VS Code', duration_secs: 9_000, event_count: 40, frame_count: 12 },
    { name: 'Chrome', duration_secs: 4_000, event_count: 20, frame_count: 8 },
  ],
  frames_captured: 20,
  events_logged: 60,
}

function digest(overrides: Partial<DailyDigestResponse> = {}): DailyDigestResponse {
  return {
    date: '2026-11-10',
    insight: null,
    timeline: [
      {
        segment_id: 'seg-1',
        start_time: '2026-11-10T09:00:00Z',
        end_time: '2026-11-10T09:48:00Z',
        duration_mins: 48,
        regime_label: 'Deep Work',
        regime_color: '#000000',
        dominant_app: 'VS Code',
        content_summary: [],
      },
    ],
    statistics: {
      deep_work_hours: 2,
      communication_hours: 0.5,
      meeting_hours: 0,
      context_switches: 14,
      longest_focus_mins: 48,
      longest_focus_content: '',
      regime_distribution: {},
    },
    generated_at: '2026-11-10T10:00:00Z',
    digest_provenance: 'heuristic',
    // What the desktop backend sends for today without an AI provider.
    ai_narrative: { failure_reason: 'not_generated' },
    ...overrides,
  }
}

function context(overrides: Partial<DashboardContext> = {}): DashboardContext {
  return {
    status: 'connected',
    // A live CPU sample is not captured activity (#13735): the empty state
    // must still win over a row of zeros.
    latestMetrics: {
      timestamp: '2026-11-10T10:00:00Z',
      cpu_usage: 12.5,
      memory_percent: 40,
      memory_used: 4e9,
      memory_total: 8e9,
    },
    idleState: null,
    metricsHistory: [],
    summary: ACTIVE_SUMMARY,
    summaryLoading: false,
    summaryError: null,
    refetchSummary: vi.fn(),
    selectedDate: '2026-11-10',
    isToday: true,
    dateRange: {},
    handleRangeChange: vi.fn(),
    isWidgetVisible: () => true,
    ...overrides,
  }
}

function renderOverview(ctx: DashboardContext) {
  return renderWithProviders(
    <Routes>
      <Route element={<Outlet context={ctx} />}>
        <Route path="/overview" element={<OverviewSection />} />
      </Route>
      <Route path="/privacy/data" element={<div data-testid="privacy-data-screen" />} />
      <Route path="/support" element={<div data-testid="support-screen" />} />
    </Routes>,
    { routerProps: { initialEntries: ['/overview'] } },
  )
}

beforeEach(() => {
  captureStatus.current = null
  vi.mocked(fetchDailyDigest).mockReset()
  vi.mocked(fetchDailyDigest).mockResolvedValue(digest())
})

describe('OverviewSection — a response without activity', () => {
  it('never draws a summary or numbers for a day with nothing captured', async () => {
    // Even a digest that carries an AI narrative and analyzed numbers must not
    // turn an empty day into a summary.
    vi.mocked(fetchDailyDigest).mockResolvedValue(
      digest({ ai_narrative: { text: 'A productive day.', provider_class: 'external_api' } }),
    )
    renderOverview(context({ summary: EMPTY_SUMMARY }))

    const firstRun = await screen.findByTestId('overview-first-run')
    // Let the digest response land before judging what was drawn.
    await waitFor(() => expect(fetchDailyDigest).toHaveBeenCalled())
    await act(() => new Promise((resolve) => setTimeout(resolve, 20)))
    expect(screen.queryByTestId('today-summary')).not.toBeInTheDocument()
    expect(screen.queryByTestId('today-summary-headline')).not.toBeInTheDocument()
    expect(screen.queryByTestId('overview-key-numbers')).not.toBeInTheDocument()
    expect(screen.queryByTestId('metric-card-active-time')).not.toBeInTheDocument()
    expect(document.body).not.toHaveTextContent('A productive day.')
    // It names what fills the screen and the next step instead.
    expect(firstRun).toHaveTextContent(i18n.t('dashboard.firstRun.guideSummary'))
    expect(firstRun).toHaveTextContent(i18n.t('dashboard.firstRun.guideNextActions'))
  })

  it('sends a profile without capture consent to the consent controls', () => {
    captureStatus.current = { paused: false, indicator_visible: true, consent_granted: false, permitted: false }
    renderOverview(context({ summary: EMPTY_SUMMARY }))

    const firstRun = screen.getByTestId('overview-first-run')
    expect(firstRun).toHaveAttribute('data-capture-state', 'consentRequired')
    expect(firstRun).toHaveTextContent(i18n.t('emptyState.dashboard.consentRequired.title'))
    fireEvent.click(within(firstRun).getByRole('button', { name: i18n.t('dashboard.firstRun.action.reviewConsent') }))
    expect(screen.getByTestId('privacy-data-screen')).toBeInTheDocument()
  })

  it('sends an unknown capture state to Support', () => {
    renderOverview(context({ summary: EMPTY_SUMMARY }))

    const firstRun = screen.getByTestId('overview-first-run')
    expect(firstRun).toHaveAttribute('data-capture-state', 'unavailable')
    fireEvent.click(within(firstRun).getByRole('button', { name: i18n.t('dashboard.firstRun.action.checkStatus') }))
    expect(screen.getByTestId('support-screen')).toBeInTheDocument()
  })

  it('offers no button while capture is running and only needs time', () => {
    captureStatus.current = { paused: false, indicator_visible: true, consent_granted: true, permitted: true }
    renderOverview(context({ summary: EMPTY_SUMMARY }))

    const firstRun = screen.getByTestId('overview-first-run')
    expect(firstRun).toHaveAttribute('data-capture-state', 'active')
    expect(within(firstRun).queryByRole('button')).not.toBeInTheDocument()
  })

  it('treats an empty past day as a quiet day, not a first run', () => {
    renderOverview(context({ summary: EMPTY_SUMMARY, selectedDate: '2026-11-03', isToday: false }))

    expect(screen.getByTestId('overview-no-activity-for-date')).toBeInTheDocument()
    expect(screen.queryByTestId('overview-first-run')).not.toBeInTheDocument()
  })
})

describe('OverviewSection — first screen with activity', () => {
  it('shows the rule-based one-line summary with its provenance when no AI is connected', async () => {
    renderOverview(context())

    const headline = await screen.findByTestId('today-summary-headline')
    expect(headline).toHaveTextContent(
      i18n.t('dashboard.todaySummary.headline.focus', { app: 'VS Code', longestFocus: formatDuration(48 * 60, true) }),
    )
    const provenance = screen.getByTestId('today-summary-provenance')
    expect(provenance).toHaveAttribute('data-provenance', 'heuristic')
    expect(provenance).toHaveTextContent(i18n.t('summaryProvenance.digest.heuristic'))
    expect(screen.getByTestId('today-summary')).toHaveTextContent(i18n.t('dashboard.todaySummary.title'))
  })

  it('orders the screen as summary, next up, key numbers', async () => {
    renderOverview(context())
    await screen.findByTestId('today-summary-headline')

    const order = ['today-summary', 'next-actions', 'overview-key-numbers'].map((id) => screen.getByTestId(id))
    for (let index = 1; index < order.length; index += 1) {
      expect(order[index - 1].compareDocumentPosition(order[index]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    }
  })

  it('shows active time exactly once, in the short format', async () => {
    renderOverview(context())
    await screen.findByTestId('today-summary-headline')

    const short = formatDuration(ACTIVE_SUMMARY.total_active_secs, true)
    // Exact text, so the card cannot carry the long "6h 7m 0s" form either.
    expect(screen.getByTestId('metric-card-active-time-value').textContent).toBe(short)
    // Once on the whole screen. Any second rendering, short or long, contains
    // the short text, so this also holds after a single format lands (#13740).
    expect(document.body.textContent?.split(short)).toHaveLength(2)
  })

  it('fills deep work and context switches from the heuristic digest', async () => {
    renderOverview(context())

    // Expected text comes from the shared formatter, so a later change to the one
    // duration format (#13740) does not need these assertions rewritten.
    await waitFor(() =>
      expect(screen.getByTestId('metric-card-deep-work-value').textContent).toBe(formatDuration(2 * 3600, true)),
    )
    expect(screen.getByTestId('metric-card-context-switches-value')).toHaveTextContent('14')
    expect(screen.getByTestId('metric-card-idle-time-value').textContent).toBe(
      formatDuration(ACTIVE_SUMMARY.total_idle_secs, true),
    )
  })

  it('leaves digest numbers blank with a reason until analysis has run', async () => {
    vi.mocked(fetchDailyDigest).mockResolvedValue(digest({ timeline: [] }))
    renderOverview(context())

    const headline = await screen.findByTestId('today-summary-headline')
    expect(headline).toHaveTextContent(i18n.t('dashboard.todaySummary.headline.app', { app: 'VS Code' }))
    const deepWork = screen.getByTestId('metric-card-deep-work')
    expect(within(deepWork).getByTestId('metric-card-deep-work-value')).toHaveTextContent('—')
    expect(deepWork).toHaveTextContent(i18n.t('dashboard.keyNumbers.pendingAnalysis'))
  })

  it('keeps the heuristic headline when the digest request fails', async () => {
    vi.mocked(fetchDailyDigest).mockRejectedValue(new Error('digest down'))
    renderOverview(context())

    const headline = await screen.findByTestId('today-summary-headline', {}, { timeout: 4000 })
    expect(headline).toHaveTextContent(i18n.t('dashboard.todaySummary.headline.app', { app: 'VS Code' }))
    expect(screen.getByTestId('metric-card-deep-work')).toHaveTextContent(i18n.t('dashboard.keyNumbers.unavailable'))
  })

  it('shows a persisted AI narrative with its provider class', async () => {
    vi.mocked(fetchDailyDigest).mockResolvedValue(
      digest({
        ai_narrative: {
          text: 'Mostly refactoring the sync layer.',
          provider_class: 'loopback',
          generated_at: '2026-11-10T09:30:00Z',
        },
      }),
    )
    renderOverview(context())

    const headline = await screen.findByTestId('today-summary-headline')
    expect(headline).toHaveTextContent('Mostly refactoring the sync layer.')
    const provenance = screen.getByTestId('today-summary-provenance')
    expect(provenance).toHaveAttribute('data-provenance', 'ai')
    expect(provenance).toHaveTextContent(i18n.t('summaryProvenance.provider.loopback'))
  })

  it('does not claim AI for a whitespace narrative or a legacy insight (#11738)', async () => {
    vi.mocked(fetchDailyDigest).mockResolvedValue(
      digest({
        insight: { narrative: 'Legacy narrative text', highlights: [] },
        ai_narrative: { text: '   ' },
      }),
    )
    renderOverview(context())

    await screen.findByTestId('today-summary-headline')
    expect(screen.getByTestId('today-summary-provenance')).toHaveAttribute('data-provenance', 'heuristic')
    expect(document.body).not.toHaveTextContent('Legacy narrative text')
  })

  it('titles a past day by its date', async () => {
    renderOverview(context({ selectedDate: '2026-11-03', isToday: false }))
    await screen.findByTestId('today-summary-headline')

    const date = new Date(2026, 10, 3).toLocaleDateString(i18n.resolvedLanguage, { month: 'long', day: 'numeric' })
    const card = screen.getByTestId('today-summary')
    expect(card).toHaveTextContent(i18n.t('dashboard.todaySummary.titleForDate', { date }))
    expect(card).not.toHaveTextContent(i18n.t('dashboard.todaySummary.title'))
  })

  it('omits a block the user hid in the widget menu', async () => {
    renderOverview(context({ isWidgetVisible: (id) => id !== 'overview.next-actions' }))
    await screen.findByTestId('today-summary-headline')

    expect(screen.queryByTestId('next-actions')).not.toBeInTheDocument()
    expect(screen.getByTestId('overview-key-numbers')).toBeInTheDocument()
  })
})

describe('OverviewSection — loading and errors', () => {
  it('shows a skeleton while the summary loads', () => {
    renderOverview(context({ summary: undefined, summaryLoading: true }))
    expect(screen.getByTestId('overview-loading')).toBeInTheDocument()
    expect(screen.queryByTestId('overview-first-run')).not.toBeInTheDocument()
  })

  it('reports a failed summary with a retry instead of an empty first screen', () => {
    const refetchSummary = vi.fn()
    renderOverview(context({ summary: undefined, summaryError: new Error('offline'), refetchSummary }))

    const alert = screen.getByTestId('overview-summary-error')
    expect(screen.queryByTestId('overview-first-run')).not.toBeInTheDocument()
    fireEvent.click(within(alert).getByRole('button'))
    expect(refetchSummary).toHaveBeenCalledTimes(1)
  })
})
