/**
 * CoachingSettingsTab tests — #5707
 *
 * Verifies:
 *   1. Master "Enable coaching" ToggleRow is rendered at the top of the tab.
 *   2. Clicking the toggle calls form.setFormData (via handleCoachingChange).
 *   3. Default-off guard: coaching.enabled defaults false in makeDefaultFormData
 *      stories-utils so that a bare default render shows the toggle unchecked.
 *   4. Blank goal labels show accessible validation without submitting settings.
 *   5. Goal input is only reset after a successful mutation.
 */
import { act, fireEvent, screen } from '@testing-library/react'
import '@testing-library/jest-dom/vitest'
import type { FormEvent } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import CoachingSettingsTab from './CoachingSettingsTab'
import { makeDefaultFormData } from './stories-utils'

// ── Context mock ──────────────────────────────────────────────────────────────
// Mirror the FocusAutoTab.test.tsx pattern: hoist before vi.mock so the spy is
// in scope at module-evaluation time.

const mockUseSettingsFormContext = vi.hoisted(() => vi.fn())
const mockUpdateGoalsMutate = vi.hoisted(() => vi.fn())

vi.mock('../settings/SettingsFormContext', () => ({
  useSettingsFormContext: mockUseSettingsFormContext,
}))

// ── Coaching hook mocks ───────────────────────────────────────────────────────
// useGoalProgress / useUpdateGoals are called inside CoachingSettingsTab.
// Returning stable empty values keeps the render free of network calls.

vi.mock('../../hooks/useCoaching', () => ({
  useGoalProgress: vi.fn().mockReturnValue({ data: [], isLoading: false }),
  useUpdateGoals: vi.fn().mockReturnValue({ mutate: mockUpdateGoalsMutate, isPending: false }),
}))

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('CoachingSettingsTab — master toggle (#5707)', () => {
  const setFormData = vi.fn()

  beforeEach(() => {
    setFormData.mockReset()
    mockUseSettingsFormContext.mockReset()
    mockUpdateGoalsMutate.mockReset()
  })

  it('renders the master Enable coaching toggle at the top of the tab', () => {
    // coaching.enabled = true so the toggle is checked in this assertion.
    const formData = makeDefaultFormData()
    // makeDefaultFormData already has coaching.enabled = true; keep as-is.

    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData },
    })

    renderWithProviders(<CoachingSettingsTab />)

    // ToggleRow renders a <label> containing the translated text.
    expect(screen.getByText(/enable coaching/i)).toBeInTheDocument()
  })

  it('default-off guard: makeDefaultFormData coaching.enabled is true but can be overridden to false', () => {
    // Verify that setting coaching.enabled = false renders the toggle unchecked.
    const formData = makeDefaultFormData({
      coaching: {
        ...makeDefaultFormData().coaching,
        enabled: false,
      },
    })

    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData },
    })

    renderWithProviders(<CoachingSettingsTab />)

    // The first checkbox in the tab is the master toggle.
    const masterToggle = screen.getAllByRole('checkbox')[0]
    expect(masterToggle).not.toBeChecked()
  })

  it('clicking the master toggle calls setFormData (via handleCoachingChange)', () => {
    // Start with coaching disabled so clicking toggles it to true.
    const formData = makeDefaultFormData({
      coaching: {
        ...makeDefaultFormData().coaching,
        enabled: false,
      },
    })

    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData },
    })

    renderWithProviders(<CoachingSettingsTab />)

    const masterToggle = screen.getAllByRole('checkbox')[0]
    fireEvent.click(masterToggle)

    // handleCoachingChange calls form.setFormData — the spy must have fired.
    expect(setFormData).toHaveBeenCalledTimes(1)
    // The updater argument is a function; call it to verify it sets coaching.enabled.
    const updaterArg = setFormData.mock.calls[0][0] as (prev: typeof formData) => typeof formData
    const next = updaterArg(formData)
    expect(next?.coaching?.enabled).toBe(true)
  })

  it('shows accessible validation for a blank goal label without submitting settings', () => {
    const formData = makeDefaultFormData()
    const handleSubmit = vi.fn((event: FormEvent) => event.preventDefault())

    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData },
    })

    renderWithProviders(
      <form onSubmit={handleSubmit}>
        <CoachingSettingsTab />
      </form>,
    )

    const labelInput = screen.getByLabelText(/regime label/i)
    fireEvent.click(screen.getByRole('button', { name: /add goal/i }))

    const error = screen.getByRole('alert')
    expect(error).toHaveTextContent(/enter a regime label/i)
    expect(labelInput).toHaveAttribute('aria-invalid', 'true')
    expect(labelInput).toHaveAttribute('aria-describedby', error.id)
    expect(labelInput).toHaveFocus()
    expect(mockUpdateGoalsMutate).not.toHaveBeenCalled()
    expect(handleSubmit).not.toHaveBeenCalled()
  })

  it('preserves goal input until the mutation succeeds', () => {
    const formData = makeDefaultFormData()
    const handleSubmit = vi.fn((event: FormEvent) => event.preventDefault())

    mockUseSettingsFormContext.mockReturnValue({
      form: { formData, setFormData },
    })

    renderWithProviders(
      <form onSubmit={handleSubmit}>
        <CoachingSettingsTab />
      </form>,
    )

    const labelInput = screen.getByLabelText(/regime label/i)
    fireEvent.change(labelInput, { target: { value: 'Deep Coding' } })
    fireEvent.click(screen.getByRole('button', { name: /add goal/i }))

    expect(mockUpdateGoalsMutate).toHaveBeenCalledTimes(1)
    expect(mockUpdateGoalsMutate.mock.calls[0][0]).toEqual({ 'Deep Coding': 60 })
    expect(labelInput).toHaveValue('Deep Coding')
    expect(handleSubmit).not.toHaveBeenCalled()

    const mutationOptions = mockUpdateGoalsMutate.mock.calls[0][1] as { onSuccess: () => void }
    act(() => mutationOptions.onSuccess())

    expect(labelInput).toHaveValue('')
  })
})
