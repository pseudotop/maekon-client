/**
 * TDD-red tests for TrackingScheduleSettings (A.19).
 *
 * A.20 will supply the real component that makes all 7 tests green.
 * The stub imported here renders nothing useful, so every assertion fails.
 *
 * Plan ref: §3.3 A.19 / U11 Korean i18n lock.
 */

import { act, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import i18n from '../../i18n'
import { TrackingScheduleSettings } from './TrackingScheduleSettings'

// ── fetch mock (used by tests 3 & 4) ────────────────────────────────────────

const fetchMock = vi.fn<typeof fetch>()

beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// ── helpers ──────────────────────────────────────────────────────────────────

/** A window fixture with all required fields (including days_of_week). */
const ALL_DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

/** Build a minimal 200 Response for GET /api/tracking-schedule */
function makeConfigResponse(windows: unknown[] = [], timezone = 'Local') {
  return new Response(JSON.stringify({ enabled: false, windows, timezone }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

/** Build a window fixture with all required backend fields. */
function makeWindow(start: string, end: string, label = '', days = ALL_DAYS) {
  return { start, end, days_of_week: days, label }
}

/** Build a 200 Response for GET /api/tracking-schedule/status */
function makeStatusResponse(activeNow: boolean, endsAt: string | null = null) {
  return new Response(
    JSON.stringify({
      active_now: activeNow,
      ends_at: endsAt,
      next_starts_at: null,
      label: '',
    }),
    { status: 200, headers: { 'Content-Type': 'application/json' } },
  )
}

// ── Test 1 — empty state ─────────────────────────────────────────────────────

describe('TrackingScheduleSettings', () => {
  it('renders tracking schedule guidance before the window editor', async () => {
    fetchMock.mockResolvedValueOnce(makeConfigResponse([])).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Tracking schedule guide' })).toBeInTheDocument()
    })
    expect(screen.getByText('Add clear windows')).toBeInTheDocument()
    expect(screen.getByText('Check active-now status')).toBeInTheDocument()
  })

  it('does not render a nested form when embedded in the settings layout form', async () => {
    fetchMock.mockResolvedValueOnce(makeConfigResponse([])).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(
      <form data-testid="settings-form">
        <TrackingScheduleSettings />
      </form>,
    )

    await waitFor(() => {
      expect(screen.getByRole('region', { name: 'Tracking schedule guide' })).toBeInTheDocument()
    })
    expect(screen.getByTestId('settings-form').querySelector('form')).toBeNull()
  })

  /**
   * Test 1: Renders empty state when windows=[]
   *
   * The real component (A.20) will show "No windows configured." text and an
   * "Add window" button when the loaded config has an empty windows array.
   * The stub just renders a placeholder div — both assertions fail.
   */
  it('renders empty state when windows=[] — shows "No windows configured." and "Add window" button', async () => {
    // GET /api/tracking-schedule → empty windows
    // GET /api/tracking-schedule/status → not active
    fetchMock.mockResolvedValueOnce(makeConfigResponse([])).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    await waitFor(() => {
      expect(screen.getByText('No windows configured.')).toBeInTheDocument()
    })
    expect(screen.getByRole('button', { name: /add window/i })).toBeInTheDocument()
  })

  // ── Test 2 — Add window ───────────────────────────────────────────────────

  /**
   * Test 2: Clicking "Add window" appends a default window to form state.
   *
   * After clicking, a new window row should appear (e.g. with HH:MM inputs for
   * start and end). The stub doesn't render the button so the click can't happen
   * → test fails on the getByRole('button') assertion.
   */
  it('clicking "Add window" appends a default window row with day-of-week checkboxes', async () => {
    const user = userEvent.setup()

    fetchMock.mockResolvedValueOnce(makeConfigResponse([])).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    const addBtn = await screen.findByRole('button', { name: /add window/i })
    await user.click(addBtn)

    // After clicking, at least one window row should be visible with HH:MM inputs.
    await waitFor(() => {
      const timeInputs = screen.getAllByRole('textbox')
      expect(timeInputs.length).toBeGreaterThanOrEqual(2)
    })

    // A.20b: DOW checkboxes should appear (7 per window).
    await waitFor(() => {
      const checkboxes = screen.getAllByRole('checkbox')
      // At least 7 checkboxes: 1 enabled toggle + 7 DOW = 8 total minimum
      expect(checkboxes.length).toBeGreaterThanOrEqual(7)
    })
  })

  // ── Test 3 — PUT /api/tracking-schedule on submit ─────────────────────────

  /**
   * Test 3: Submitting a valid form calls PUT /api/tracking-schedule.
   *
   * The real component wraps a <form> with a Save button; submitting it fires a
   * React Query mutation that PUTs the config. The stub has no form, so the
   * getByRole('button', { name: /save/i }) fails.
   */
  it('submitting a valid form calls PUT /api/tracking-schedule', async () => {
    const user = userEvent.setup()

    // GET config (one pre-existing window so the form is not empty)
    fetchMock
      .mockResolvedValueOnce(makeConfigResponse([makeWindow('09:00', '17:00', 'Work')]))
      .mockResolvedValueOnce(makeStatusResponse(false))
      // PUT response
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            enabled: true,
            windows: [makeWindow('09:00', '17:00', 'Work')],
            timezone: 'Local',
          }),
          { status: 200, headers: { 'Content-Type': 'application/json' } },
        ),
      )

    renderWithProviders(<TrackingScheduleSettings />)

    const saveBtn = await screen.findByRole('button', { name: /save/i })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        expect.stringContaining('/api/tracking-schedule'),
        expect.objectContaining({ method: 'PUT' }),
      )
    })
  })

  // ── Test 4 — "Active now" pill ────────────────────────────────────────────

  /**
   * Test 4: Shows "Active now — ends HH:MM" pill when /status returns active_now=true.
   *
   * The real component polls (or fetches) /api/tracking-schedule/status and
   * conditionally renders a status pill. The stub renders none → assertion fails.
   */
  it('shows "Active now — ends HH:MM" pill when /status returns active_now=true', async () => {
    fetchMock
      .mockResolvedValueOnce(makeConfigResponse([makeWindow('09:00', '17:00', 'Work')]))
      .mockResolvedValueOnce(makeStatusResponse(true, '2026-04-24T17:00:00+09:00'))

    renderWithProviders(<TrackingScheduleSettings />)

    await waitFor(() => {
      // The pill text matches "Active now — ends 17:00" (exact time from ends_at).
      expect(screen.getByText(/active now/i)).toBeInTheDocument()
    })
  })

  // ── Test 5 — HH:MM validation ─────────────────────────────────────────────

  /**
   * Test 5: Entering "12:XX" surfaces an inline validation error.
   *
   * A.20 will validate each window's start/end against the HH:MM regex on
   * change (or on blur). The stub renders no input → test fails at findByRole.
   */
  it('entering "12:XX" into a time field surfaces an inline validation error', async () => {
    const user = userEvent.setup()

    fetchMock
      .mockResolvedValueOnce(makeConfigResponse([makeWindow('09:00', '17:00', 'Work')]))
      .mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    // Find the start-time input for the first window.
    const startInput = await screen.findByLabelText(/start/i)
    await user.tripleClick(startInput)
    await user.type(startInput, '12:XX')

    await waitFor(() => {
      expect(screen.getByText(/HH:MM/i)).toBeInTheDocument()
    })
  })

  // ── Test 6 — timezone dropdown ────────────────────────────────────────────

  /**
   * Test 6: Timezone input shows a dropdown/combobox with IANA names and
   * "Local" as the default selected option.
   *
   * A.20 will render a <select> (or combobox) seeded with common IANA zones
   * plus "Local". The stub has no select → assertion fails.
   */
  it('timezone input renders a combobox with IANA names and "Local" as default', async () => {
    fetchMock.mockResolvedValueOnce(makeConfigResponse([], 'Local')).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    // Expect a combobox (select or role=combobox) for timezone.
    const tzSelect = await screen.findByRole('combobox', { name: /timezone/i })
    expect(tzSelect).toBeInTheDocument()

    // The default selected value should be "Local".
    expect(tzSelect).toHaveValue('Local')

    // At least one IANA zone should be available as an option.
    const options = screen.getAllByRole('option')
    const hasIanaZone = options.some((o) =>
      /^(America|Europe|Asia|Pacific|Atlantic|Indian|Africa|Arctic|Antarctica)\//i.test(o.textContent ?? ''),
    )
    expect(hasIanaZone).toBe(true)
  })

  // ── Test 7 — Korean locale (U11 lock) ────────────────────────────────────

  /**
   * Test 7 (U11): Korean locale renders "추적 일정" as the page title, NOT "스케줄".
   *
   * This is a U11 i18n lock — ensures the Korean translation uses the canonical
   * "추적 일정" wording. The stub renders a placeholder div that contains neither
   * string → the "추적 일정" assertion fails.
   */
  it('Korean locale renders "추적 일정" title and does not render "스케줄" (U11 lock)', async () => {
    // Switch i18n to Korean for this test.
    await act(async () => {
      await i18n.changeLanguage('ko')
    })

    fetchMock.mockResolvedValueOnce(makeConfigResponse([])).mockResolvedValueOnce(makeStatusResponse(false))

    renderWithProviders(<TrackingScheduleSettings />)

    await waitFor(() => {
      expect(screen.getByText('추적 일정')).toBeInTheDocument()
    })
    // U11 negative assertion: the old "스케줄" label must NOT appear.
    expect(screen.queryByText('스케줄')).not.toBeInTheDocument()

    // Restore English for subsequent tests.
    await act(async () => {
      await i18n.changeLanguage('en')
    })
  })
})
