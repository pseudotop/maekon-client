import { act, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import { addToast, clearToasts } from '../../../hooks/useToast'
import { ToastContainer } from '../Toast'

describe('ToastContainer', () => {
  afterEach(() => {
    act(() => {
      clearToasts()
    })
    vi.useRealTimers()
  })

  it('renders a toast and allows dismissing it manually', async () => {
    const user = userEvent.setup()
    renderWithProviders(<ToastContainer />)

    act(() => {
      addToast('success', 'Settings saved.', 0)
    })

    expect(screen.getByText('Settings saved.')).toBeInTheDocument()

    await act(async () => {
      await user.click(screen.getByRole('button'))
    })

    await waitFor(() => {
      expect(screen.queryByText('Settings saved.')).not.toBeInTheDocument()
    })
  })

  it('auto-dismisses timed toasts', async () => {
    vi.useFakeTimers()
    renderWithProviders(<ToastContainer />)

    act(() => {
      addToast('info', 'Checking for updates…', 1200)
    })

    expect(screen.getByText('Checking for updates…')).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(1200)
    })

    expect(screen.queryByText('Checking for updates…')).not.toBeInTheDocument()
  })
})
