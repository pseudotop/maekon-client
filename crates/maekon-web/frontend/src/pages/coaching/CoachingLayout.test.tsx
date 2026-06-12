/**
 * CoachingLayout tests — #5707
 *
 * Verifies the disabled-state banner:
 *   - Banner is shown when settings.coaching.enabled === false.
 *   - Banner is NOT shown when settings.coaching.enabled === true.
 *   - Banner includes the "Enable coaching" action button.
 */
import { screen } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { makeDefaultFormData } from '../setting-tabs/stories-utils'

// ── api/client mock ───────────────────────────────────────────────────────────
// CoachingLayout calls fetchSettings via useQuery and updateSettings via
// useMutation. Stub both so no real HTTP requests are made.

const mockFetchSettings = vi.fn()
const mockUpdateSettings = vi.fn()

vi.mock('../../api/client', () => ({
  fetchSettings: (...args: unknown[]) => mockFetchSettings(...args),
  updateSettings: (...args: unknown[]) => mockUpdateSettings(...args),
}))

// ── Coaching hooks mock ───────────────────────────────────────────────────────
// useCoachingHistory, useGoalProgress, and useHabitStreaks (used by
// HabitTrackerWidget, which CoachingLayout renders) must all be stubbed.
// Use importOriginal to spread any other exports from the module.

vi.mock('../../hooks/useCoaching', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../hooks/useCoaching')>()
  return {
    ...actual,
    useCoachingHistory: vi.fn().mockReturnValue({ data: [], isLoading: false }),
    useGoalProgress: vi.fn().mockReturnValue({ data: [], isLoading: false }),
    useHabitStreaks: vi.fn().mockReturnValue({ data: [], isLoading: false }),
    useUpdateGoals: vi.fn().mockReturnValue({ mutate: vi.fn(), isPending: false }),
  }
})

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('CoachingLayout — disabled-state banner (#5707)', () => {
  beforeEach(() => {
    mockFetchSettings.mockReset()
    mockUpdateSettings.mockReset()
    mockUpdateSettings.mockResolvedValue(makeDefaultFormData())
  })

  it('shows the disabled banner when coaching.enabled === false', async () => {
    // Resolve fetchSettings with coaching disabled.
    mockFetchSettings.mockResolvedValue(
      makeDefaultFormData({ coaching: { ...makeDefaultFormData().coaching, enabled: false } }),
    )

    const CoachingLayout = (await import('./CoachingLayout')).default
    renderWithProviders(<CoachingLayout />, {
      routerProps: { initialEntries: ['/coaching'] },
    })

    // Wait for the settings query to resolve and the banner to appear.
    const banner = await screen.findByTestId('coaching-disabled-banner')
    expect(banner).toBeInTheDocument()
    // The banner should include the Enable button.
    expect(screen.getByRole('button', { name: /enable coaching/i })).toBeInTheDocument()
  })

  it('hides the disabled banner when coaching.enabled === true', async () => {
    // Resolve fetchSettings with coaching enabled (the default in stories-utils).
    mockFetchSettings.mockResolvedValue(makeDefaultFormData())

    const CoachingLayout = (await import('./CoachingLayout')).default
    renderWithProviders(<CoachingLayout />, {
      routerProps: { initialEntries: ['/coaching'] },
    })

    // The settings query resolves immediately; the banner must not appear.
    // findByTestId would time out — use queryByTestId to confirm absence.
    // Give React a tick to flush the query result.
    await screen.findByText(/coaching history/i)

    expect(screen.queryByTestId('coaching-disabled-banner')).not.toBeInTheDocument()
  })
})
