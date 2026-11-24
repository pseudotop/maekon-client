import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../../__tests__/helpers/render-helpers'
import { addErrorToast, addToast, clearToasts } from '../../../hooks/useToast'
import en from '../../../i18n/locales/en.json'
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

  it('keeps an error toast detail collapsed and stays while support reads it (#13737)', () => {
    vi.useFakeTimers()
    renderWithProviders(<ToastContainer />)

    act(() => {
      addErrorToast({ text: 'The tag change could not be saved.', detail: 'storage.unavailable: pool closed' }, 1200)
    })

    expect(screen.getByText('The tag change could not be saved.')).toBeInTheDocument()
    expect(screen.getByText(en.errors.supportDetails)).toBeInTheDocument()
    expect(screen.getByTestId('support-details-text')).toHaveTextContent('storage.unavailable: pool closed')

    const details = screen.getByTestId('support-details') as HTMLDetailsElement
    act(() => {
      details.open = true
      fireEvent(details, new Event('toggle'))
    })
    act(() => {
      vi.advanceTimersByTime(1200)
    })

    expect(screen.getByText('The tag change could not be saved.')).toBeInTheDocument()
  })

  it('renders no support panel for a toast without detail', () => {
    renderWithProviders(<ToastContainer />)

    act(() => {
      addErrorToast({ text: 'Failed to rename the session.', detail: null }, 0)
    })

    expect(screen.getByText('Failed to rename the session.')).toBeInTheDocument()
    expect(screen.queryByTestId('support-details')).toBeNull()
  })
})
