import { act, fireEvent, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '../../__tests__/helpers/render-helpers'
import { showToast, ToastContainer } from './Toast'

beforeEach(() => vi.useFakeTimers())
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('Overlay support details and bounded toast queue (#13756)', () => {
  it('keeps diagnostics collapsed, natively focusable and pinned after opening/copying', async () => {
    // Native disclosure focus/click is covered here; real Tab/Enter needs a browser.
    vi.useRealTimers()
    const copy = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: copy } })
    renderWithProviders(<ToastContainer />)
    act(() => showToast('Localized failure', 'error', undefined, 'future.overlay: original diagnostic'))
    const detail = screen.getByTestId('support-details') as HTMLDetailsElement
    expect(detail).not.toHaveAttribute('open')
    expect(detail.previousElementSibling).toHaveTextContent('Localized failure')
    expect(detail.previousElementSibling).not.toHaveTextContent('original diagnostic')
    expect(detail.parentElement).toHaveClass('pointer-events-auto')
    expect(detail.closest('output')).toHaveClass('pointer-events-none')
    const summary = detail.querySelector('summary')!
    summary.focus()
    expect(summary).toHaveFocus()
    fireEvent.click(summary)
    // jsdom implements the native details toggle; dispatch its notification explicitly.
    expect(detail).toHaveAttribute('open')
    fireEvent(detail, new Event('toggle'))
    await act(async () => {
      fireEvent.click(within(detail).getByRole('button'))
      await Promise.resolve()
    })
    expect(copy).toHaveBeenCalledWith('future.overlay: original diagnostic')
    vi.useFakeTimers()
    act(() => showToast('another notification', 'info'))
    await act(async () => {
      vi.advanceTimersByTime(10_000)
    })
    expect(detail).toBeInTheDocument()
    expect(screen.queryByText('another notification')).toBeNull()
    fireEvent.click(within(detail.parentElement!).getByRole('button', { name: 'Close' }))
    expect(screen.queryByTestId('support-details')).toBeNull()
  })

  it('preserves the default and explicit durations for ordinary notifications', async () => {
    renderWithProviders(<ToastContainer />)
    act(() => {
      showToast('success message', 'success')
      showToast('information', 'info', 1000)
    })
    expect(screen.queryByTestId('support-details')).toBeNull()
    await act(async () => {
      vi.advanceTimersByTime(3999)
    })
    expect(screen.getByText('success message')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(1)
    })
    expect(screen.queryByText('success message')).toBeNull()
    expect(screen.getByText('information')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(999)
    })
    expect(screen.getByText('information')).toBeInTheDocument()
    await act(async () => {
      vi.advanceTimersByTime(1)
    })
    expect(screen.queryByText('information')).toBeNull()
  })

  it('keeps three visible and twenty queued, drops only oldest pending and promotes on dismissal', async () => {
    renderWithProviders(<ToastContainer />)
    act(() => {
      for (let index = 0; index < 25; index++) showToast(`message-${index}`, 'info')
    })
    const output = screen.getByRole('status')
    expect(output.children).toHaveLength(3)
    expect(screen.getByText('message-0')).toBeInTheDocument()
    expect(screen.getByText('message-2')).toBeInTheDocument()
    await act(async () => {
      fireEvent.click(within(output.children[0] as HTMLElement).getByRole('button'))
    })
    expect(output.children).toHaveLength(3)
    expect(screen.queryByText('message-0')).toBeNull()
    expect(screen.queryByText('message-3')).toBeNull()
    expect(screen.queryByText('message-4')).toBeNull()
    expect(screen.getByText('message-5')).toBeInTheDocument()
  })

  it('lets other visible messages expire while a diagnostic remains pinned', async () => {
    renderWithProviders(<ToastContainer />)
    act(() => {
      showToast('failure', 'error', undefined, 'diagnostic')
      showToast('ordinary', 'success')
    })
    const detail = screen.getByTestId('support-details') as HTMLDetailsElement
    detail.open = true
    fireEvent(detail, new Event('toggle'))
    await act(async () => {
      vi.advanceTimersByTime(4000)
    })
    expect(screen.getByText('failure')).toBeInTheDocument()
    expect(screen.queryByText('ordinary')).toBeNull()
  })
})
